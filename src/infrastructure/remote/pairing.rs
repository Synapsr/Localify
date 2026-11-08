//! The commit and reveal that make the 6-digit pairing code safe against a
//! relay in the middle (`docs/remote-machines.md`, "Pairing"; ADR 0029).
//!
//! The responder (the target) commits to its nonce before it sees the
//! initiator's, and both nonces and the final handshake hash go into the
//! code. The messages come in exactly one order, each once: commit, nonce,
//! reveal, then the two confirms. Anything else abandons the pairing, and a
//! side reports [`Step::Paired`] only once its own person accepted and the
//! peer's acceptance arrived.

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use blake2::{Blake2s256, Digest};
use subtle_compare::constant_time_eq;

use super::noise::HASH_LEN;
use super::wire::SessionMessage;
use crate::infrastructure::launch_rev::random_bytes;

pub const NONCE_LEN: usize = 32;
const COMMIT_LABEL: &[u8] = b"localify-pair-commit";
const CODE_LABEL: &[u8] = b"localify-pair-code";

pub type Nonce = [u8; NONCE_LEN];

/// `BLAKE2s("localify-pair-commit" ‖ h ‖ nB)`.
pub fn commitment(h: &[u8; HASH_LEN], responder_nonce: &Nonce) -> [u8; 32] {
    let mut hasher = Blake2s256::new();
    hasher.update(COMMIT_LABEL);
    hasher.update(h);
    hasher.update(responder_nonce);
    hasher.finalize().into()
}

/// The first 4 bytes of `BLAKE2s("localify-pair-code" ‖ h ‖ nA ‖ nB)`, big
/// endian, modulo 1,000,000.
pub fn code(h: &[u8; HASH_LEN], initiator_nonce: &Nonce, responder_nonce: &Nonce) -> u32 {
    let mut hasher = Blake2s256::new();
    hasher.update(CODE_LABEL);
    hasher.update(h);
    hasher.update(initiator_nonce);
    hasher.update(responder_nonce);
    let digest = hasher.finalize();
    u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000
}

/// Two groups of three digits: `482 913`.
pub fn format_code(code: u32) -> String {
    format!("{:03} {:03}", code / 1000, code % 1000)
}

fn fresh_nonce() -> Result<Nonce> {
    let mut nonce = [0_u8; NONCE_LEN];
    random_bytes(&mut nonce)?;
    Ok(nonce)
}

fn encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn decode32(text: &str) -> Option<[u8; 32]> {
    URL_SAFE_NO_PAD.decode(text).ok()?.try_into().ok()
}

/// What the caller does after a step of the pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Send this message in the session.
    Send(SessionMessage),
    /// Show this code to the person, and wait for an answer.
    ShowCode(String),
    /// Both accepted: pin the peer's key.
    Paired,
    /// Either declined: pin nothing.
    Cancelled,
}

/// Why a pairing was abandoned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairingError {
    #[error("the pairing message {0} came out of order or twice")]
    OutOfOrder(&'static str),
    #[error("the revealed nonce does not match the commitment")]
    CommitmentMismatch,
    #[error("the {0} is not 32 bytes of base64url")]
    Malformed(&'static str),
    #[error("the peer sent a nonce this pairing has already used")]
    NonceReuse,
    #[error("the person already answered")]
    AlreadyAnswered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Initiator,
    Responder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// Initiator: waiting for `pair.commit`.
    AwaitCommit,
    /// Initiator: sent `pair.nonce`, waiting for `pair.reveal`.
    AwaitReveal {
        commitment: [u8; 32],
        nonce: Nonce,
    },
    /// Responder: sent `pair.commit`, waiting for `pair.nonce`.
    Committed {
        nonce: Nonce,
    },
    /// Both: the code is shown, waiting for both answers.
    Confirming {
        mine: Option<bool>,
        theirs: Option<bool>,
    },
    Finished,
}

/// One side of one pairing, from the final handshake hash on.
#[derive(Debug, Clone)]
pub struct Pairing {
    side: Side,
    h: [u8; HASH_LEN],
    state: State,
}

impl Pairing {
    /// The viewer, which waits for the target's commitment.
    pub fn initiator(h: [u8; HASH_LEN]) -> Self {
        Self {
            side: Side::Initiator,
            h,
            state: State::AwaitCommit,
        }
    }

    /// The target, which commits to a fresh nonce at once. The commitment
    /// is the message to send.
    pub fn responder(h: [u8; HASH_LEN]) -> Result<(Self, SessionMessage)> {
        let nonce = fresh_nonce()?;
        let c = commitment(&h, &nonce);
        let pairing = Self {
            side: Side::Responder,
            h,
            state: State::Committed { nonce },
        };
        Ok((pairing, SessionMessage::PairCommit { c: encode(&c) }))
    }

    pub fn is_finished(&self) -> bool {
        self.state == State::Finished
    }

    /// Takes the peer's next pairing message.
    pub fn receive(&mut self, message: &SessionMessage) -> Result<Vec<Step>, PairingError> {
        let state = std::mem::replace(&mut self.state, State::Finished);
        let (state, steps) = self.next(state, message)?;
        self.state = state;
        Ok(steps)
    }

    fn next(
        &self,
        state: State,
        message: &SessionMessage,
    ) -> Result<(State, Vec<Step>), PairingError> {
        match (self.side, state, message) {
            (Side::Initiator, State::AwaitCommit, SessionMessage::PairCommit { c }) => {
                let commitment = decode32(c).ok_or(PairingError::Malformed("commitment"))?;
                let nonce = fresh_nonce().map_err(|_| PairingError::Malformed("nonce"))?;
                let send = SessionMessage::PairNonce { n: encode(&nonce) };
                Ok((
                    State::AwaitReveal { commitment, nonce },
                    vec![Step::Send(send)],
                ))
            }
            (Side::Responder, State::Committed { nonce }, SessionMessage::PairNonce { n }) => {
                let theirs = decode32(n).ok_or(PairingError::Malformed("nonce"))?;
                if constant_time_eq(&theirs, &nonce) {
                    return Err(PairingError::NonceReuse);
                }
                let shown = format_code(code(&self.h, &theirs, &nonce));
                let reveal = SessionMessage::PairReveal { n: encode(&nonce) };
                Ok((
                    confirming(),
                    vec![Step::Send(reveal), Step::ShowCode(shown)],
                ))
            }
            (
                Side::Initiator,
                State::AwaitReveal {
                    commitment: c,
                    nonce,
                },
                SessionMessage::PairReveal { n },
            ) => {
                let theirs = decode32(n).ok_or(PairingError::Malformed("nonce"))?;
                if !constant_time_eq(&commitment(&self.h, &theirs), &c) {
                    return Err(PairingError::CommitmentMismatch);
                }
                if constant_time_eq(&theirs, &nonce) {
                    return Err(PairingError::NonceReuse);
                }
                let shown = format_code(code(&self.h, &nonce, &theirs));
                Ok((confirming(), vec![Step::ShowCode(shown)]))
            }
            (
                _,
                State::Confirming { mine, theirs: None },
                SessionMessage::PairConfirm { accept },
            ) => Ok(settle(mine, Some(*accept), vec![])),
            (_, _, message) => Err(PairingError::OutOfOrder(name(message))),
        }
    }

    /// The person's answer, once the code is shown.
    pub fn answer(&mut self, accept: bool) -> Result<Vec<Step>, PairingError> {
        let State::Confirming { mine: None, theirs } = self.state else {
            return Err(PairingError::AlreadyAnswered);
        };
        let send = vec![Step::Send(SessionMessage::PairConfirm { accept })];
        let (state, steps) = settle(Some(accept), theirs, send);
        self.state = state;
        Ok(steps)
    }
}

fn confirming() -> State {
    State::Confirming {
        mine: None,
        theirs: None,
    }
}

fn settle(mine: Option<bool>, theirs: Option<bool>, mut steps: Vec<Step>) -> (State, Vec<Step>) {
    match (mine, theirs) {
        (Some(false), _) | (_, Some(false)) => {
            steps.push(Step::Cancelled);
            (State::Finished, steps)
        }
        (Some(true), Some(true)) => {
            steps.push(Step::Paired);
            (State::Finished, steps)
        }
        _ => (State::Confirming { mine, theirs }, steps),
    }
}

fn name(message: &SessionMessage) -> &'static str {
    match message {
        SessionMessage::PairCommit { .. } => "pair.commit",
        SessionMessage::PairNonce { .. } => "pair.nonce",
        SessionMessage::PairReveal { .. } => "pair.reveal",
        SessionMessage::PairConfirm { .. } => "pair.confirm",
        SessionMessage::PairDeclined { .. } => "pair.declined",
        _ => "a non-pairing message",
    }
}

/// Comparing secrets byte by byte would leak how many bytes matched.
mod subtle_compare {
    pub fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
        let difference = a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y));
        std::hint::black_box(difference) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a pairing to the shown codes; returns both sides and their codes.
    fn to_codes(h_a: [u8; 32], h_b: [u8; 32]) -> (Pairing, Pairing, String, String) {
        let mut a = Pairing::initiator(h_a);
        let (mut b, commit) = Pairing::responder(h_b).unwrap();
        let steps = a.receive(&commit).unwrap();
        let [Step::Send(nonce)] = steps.as_slice() else {
            panic!("{steps:?}");
        };
        let steps = b.receive(nonce).unwrap();
        let [Step::Send(reveal), Step::ShowCode(code_b)] = steps.as_slice() else {
            panic!("{steps:?}");
        };
        let steps = a.receive(reveal).unwrap();
        let [Step::ShowCode(code_a)] = steps.as_slice() else {
            panic!("{steps:?}");
        };
        let (code_a, code_b) = (code_a.clone(), code_b.clone());
        (a, b, code_a, code_b)
    }

    fn confirm(accept: bool) -> SessionMessage {
        SessionMessage::PairConfirm { accept }
    }

    #[test]
    fn both_sides_show_the_same_code_and_pair_once_both_accept() {
        let (mut a, mut b, code_a, code_b) = to_codes([1; 32], [1; 32]);
        assert_eq!(code_a, code_b);
        assert_eq!(code_a.len(), 7);

        let steps = a.answer(true).unwrap();
        assert_eq!(steps, vec![Step::Send(confirm(true))]);
        let steps = b.receive(&confirm(true)).unwrap();
        assert!(steps.is_empty(), "B's person has not answered: {steps:?}");
        let steps = b.answer(true).unwrap();
        assert_eq!(steps, vec![Step::Send(confirm(true)), Step::Paired]);
        assert_eq!(a.receive(&confirm(true)).unwrap(), vec![Step::Paired]);
        assert!(a.is_finished() && b.is_finished());
    }

    #[test]
    fn a_cancel_on_either_side_pins_nothing() {
        let (mut a, mut b, _, _) = to_codes([2; 32], [2; 32]);
        assert_eq!(
            a.answer(false).unwrap(),
            vec![Step::Send(confirm(false)), Step::Cancelled]
        );
        b.answer(true).unwrap();
        assert_eq!(b.receive(&confirm(false)).unwrap(), vec![Step::Cancelled]);

        let (mut a, _, _, _) = to_codes([3; 32], [3; 32]);
        a.answer(true).unwrap();
        assert_eq!(a.receive(&confirm(false)).unwrap(), vec![Step::Cancelled]);
    }

    #[test]
    fn a_nonce_before_the_commitment_abandons_the_pairing() {
        let mut a = Pairing::initiator([4; 32]);
        let reveal = SessionMessage::PairReveal {
            n: encode(&[0; 32]),
        };
        assert!(matches!(
            a.receive(&reveal),
            Err(PairingError::OutOfOrder(_))
        ));

        // The responder never takes a commitment, and takes the nonce once.
        let (mut b, commit) = Pairing::responder([4; 32]).unwrap();
        assert!(b.receive(&commit).is_err());
        let (mut b, _) = Pairing::responder([4; 32]).unwrap();
        let nonce = SessionMessage::PairNonce {
            n: encode(&[9; 32]),
        };
        b.receive(&nonce).unwrap();
        assert!(b.receive(&nonce).is_err());
    }

    #[test]
    fn a_duplicated_commitment_abandons_the_pairing() {
        let mut a = Pairing::initiator([5; 32]);
        let (_, commit) = Pairing::responder([5; 32]).unwrap();
        a.receive(&commit).unwrap();
        assert!(matches!(
            a.receive(&commit),
            Err(PairingError::OutOfOrder("pair.commit"))
        ));
    }

    #[test]
    fn a_reveal_that_does_not_match_the_commitment_is_refused() {
        let mut a = Pairing::initiator([6; 32]);
        let (_, commit) = Pairing::responder([6; 32]).unwrap();
        a.receive(&commit).unwrap();
        let other = SessionMessage::PairReveal {
            n: encode(&[1; 32]),
        };
        assert_eq!(a.receive(&other), Err(PairingError::CommitmentMismatch));
    }

    #[test]
    fn a_reflected_nonce_is_refused() {
        let (mut b, commit) = Pairing::responder([7; 32]).unwrap();
        // The relay cannot read nB, but a peer that echoes a commitment's
        // preimage would: model it with the responder's own nonce.
        let State::Committed { nonce } = b.state.clone() else {
            panic!("not committed");
        };
        let echoed = SessionMessage::PairNonce { n: encode(&nonce) };
        assert_eq!(b.receive(&echoed), Err(PairingError::NonceReuse));
        drop(commit);
    }

    #[test]
    fn a_repeated_confirm_or_a_second_answer_is_refused() {
        let (mut a, _, _, _) = to_codes([8; 32], [8; 32]);
        a.receive(&confirm(true)).unwrap();
        assert!(a.receive(&confirm(true)).is_err());
        assert!(a.is_finished());

        let (mut a, _, _, _) = to_codes([8; 32], [8; 32]);
        a.answer(true).unwrap();
        assert_eq!(a.answer(true), Err(PairingError::AlreadyAnswered));
    }

    #[test]
    fn a_confirm_before_the_code_is_refused() {
        let mut a = Pairing::initiator([9; 32]);
        assert!(a.receive(&confirm(true)).is_err());
        assert_eq!(a.answer(true), Err(PairingError::AlreadyAnswered));
    }

    #[test]
    fn an_altered_handshake_hash_abandons_the_pairing() {
        // A relay that substitutes a key leaves the two sides with
        // different hashes, so the commitment B made does not check on A.
        let mut a = Pairing::initiator([10; 32]);
        let (mut b, commit) = Pairing::responder([11; 32]).unwrap();
        let Step::Send(nonce) = &a.receive(&commit).unwrap()[0] else {
            panic!()
        };
        let Step::Send(reveal) = &b.receive(nonce).unwrap()[0] else {
            panic!()
        };
        assert_eq!(a.receive(reveal), Err(PairingError::CommitmentMismatch));
        assert!(a.is_finished());
    }

    /// A relay running two handshakes is A's responder and B's initiator.
    /// It must commit to A before it learns B's nonce, and B commits before
    /// it learns the relay's, so it cannot steer both codes to one value.
    #[test]
    fn a_relay_in_the_middle_cannot_make_both_codes_match() {
        let mut matches = 0;
        for attempt in 0..200_u8 {
            let (h_a, h_b) = ([attempt; 32], [attempt.wrapping_add(1); 32]);
            // Relay ↔ A, with the relay as the responder.
            let mut a = Pairing::initiator(h_a);
            let (mut relay_as_b, commit_to_a) = Pairing::responder(h_a).unwrap();
            // Relay ↔ B, with the relay as the initiator.
            let mut relay_as_a = Pairing::initiator(h_b);
            let (mut b, commit_to_relay) = Pairing::responder(h_b).unwrap();

            let Step::Send(nonce_a) = &a.receive(&commit_to_a).unwrap()[0] else {
                panic!()
            };
            let Step::Send(nonce_relay) = &relay_as_a.receive(&commit_to_relay).unwrap()[0] else {
                panic!()
            };
            let steps_b = b.receive(nonce_relay).unwrap();
            let steps_relay_b = relay_as_b.receive(nonce_a).unwrap();
            let Step::ShowCode(code_b) = &steps_b[1] else {
                panic!()
            };
            let Step::Send(reveal_to_a) = &steps_relay_b[0] else {
                panic!()
            };
            let Step::ShowCode(code_a) = &a.receive(reveal_to_a).unwrap()[0] else {
                panic!()
            };
            if code_a == code_b {
                matches += 1;
            }
        }
        // Each attempt matches with probability 10⁻⁶.
        assert!(matches <= 1, "{matches} of 200 attempts matched");
    }

    #[test]
    fn codes_are_six_digits_in_two_groups() {
        assert_eq!(format_code(482_913), "482 913");
        assert_eq!(format_code(7), "000 007");
        let value = code(&[0; 32], &[1; 32], &[2; 32]);
        assert!(value < 1_000_000);
    }
}
