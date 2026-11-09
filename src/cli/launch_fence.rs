//! Launch fencing in `localify dev` (docs/remote-machines.md, "The run lock
//! and launch fencing"). A dispatcher names its launch, `--launch-id`, or
//! `--remote-launch` for a control from a paired machine, and may give it an
//! expiry and the revision it checked. Under its run lock, the executor checks
//! the launch against the definition it loaded, publishes its decision in the
//! launch record, and runs only once it admitted it. A person's own launch has
//! no launch record and nothing to check.

use anyhow::{Error, Result, anyhow, bail};

use crate::infrastructure::launch_profiles::{LaunchProfiles, allowed_remotely};
use crate::infrastructure::process::{self, helpers};
use crate::infrastructure::running_services::{RunningServiceStore, ServiceKey, ServiceOrigin};
use crate::infrastructure::runs::launches::{
    AdmittedLaunch, ExecutorIdentity, ExitReason, LaunchExit, LaunchLock, LaunchRecord,
    Publication, RefusalCode, RequestedBy, record_alive, unix_ms_now,
};
use crate::infrastructure::runs::{RunId, output};
use crate::infrastructure::test_pause::pause_at;

/// The hidden flags of `localify dev` that a dispatcher passes.
#[derive(Debug, Default, Clone)]
pub struct DispatchFlags {
    pub launch_id: Option<String>,
    pub remote_launch: Option<String>,
    pub launch_expires: Option<u64>,
    pub expect_rev: Option<String>,
    pub request: Option<String>,
}

impl DispatchFlags {
    /// The launch a dispatcher named, or `None` for a person's own.
    pub fn parse(self) -> Result<Option<Dispatch>> {
        let remote = self.remote_launch.is_some();
        let id = match (self.launch_id, self.remote_launch) {
            (Some(_), Some(_)) => bail!("--remote-launch is used in place of --launch-id."),
            (Some(id), None) | (None, Some(id)) => RunId::parse(&id)?,
            (None, None) => {
                if self.launch_expires.is_some()
                    || self.expect_rev.is_some()
                    || self.request.is_some()
                {
                    bail!(
                        "--launch-expires, --expect-rev and --request need --launch-id or --remote-launch."
                    );
                }
                return Ok(None);
            }
        };
        if remote
            && (self.launch_expires.is_none()
                || self.expect_rev.is_none()
                || self.request.is_none())
        {
            bail!("--remote-launch needs --launch-expires, --expect-rev and --request.");
        }
        if !remote && self.request.is_some() {
            bail!("--request names who asked for a --remote-launch.");
        }
        let requested_by = self
            .request
            .as_deref()
            .map(RequestedBy::parse)
            .transpose()?;
        Ok(Some(Dispatch {
            id,
            expires_at: self.launch_expires,
            expect_rev: self.expect_rev,
            requested_by,
            remote,
        }))
    }
}

/// Refuses the launch `flags` name, `conflict`, for an executor that stops
/// before it could read the config. Returns the error it exits with; with no
/// launch named, or one whose flags do not parse, that is `error` alone.
pub fn refuse_before_start(flags: DispatchFlags, error: Error) -> Error {
    let Ok(Some(dispatch)) = flags.parse() else {
        return error;
    };
    match RunningServiceStore::new_default() {
        Ok(store) => dispatch.refuse(&store, Refusal::conflict(error)),
        Err(_) => error,
    }
}

/// A launch whose id a dispatcher chose: the id of its run and of its launch
/// record.
#[derive(Debug, Clone)]
pub struct Dispatch {
    pub id: RunId,
    /// Unix milliseconds.
    expires_at: Option<u64>,
    expect_rev: Option<String>,
    requested_by: Option<RequestedBy>,
    /// `--remote-launch`: only a profile a control may name runs.
    remote: bool,
}

/// Why a launch is refused, with the code its launch record carries.
pub struct Refusal {
    pub code: RefusalCode,
    pub error: Error,
}

impl Refusal {
    pub fn conflict(error: Error) -> Self {
        Self {
            code: RefusalCode::Conflict,
            error,
        }
    }
}

/// What a launch is checked against: the definition the executor loaded under
/// its lock, its revision computed only when a dispatcher expects one.
pub struct Loaded<'a> {
    pub profiles: &'a Result<Option<LaunchProfiles>>,
    pub profile: Option<&'a str>,
    pub revision: &'a dyn Fn() -> Result<String>,
}

impl Dispatch {
    /// Whether this launch came from a paired machine.
    pub fn is_remote(&self) -> bool {
        self.remote
    }

    /// Whether its dispatcher checked a revision (`--expect-rev`).
    pub fn expects_revision(&self) -> bool {
        self.expect_rev.is_some()
    }

    /// Fails when this launch is already decided, such as by the
    /// reconciler's `expired` or an earlier run of the same id: it then stops
    /// nothing and starts nothing, and the decision stands.
    pub fn ensure_undecided(&self, store: &RunningServiceStore) -> Result<()> {
        match store.launches().read(&self.id)? {
            None => Ok(()),
            Some(LaunchRecord::Expired { .. }) => bail!(
                "This launch expired before it could start (launch {}); nothing was started.",
                self.id
            ),
            Some(LaunchRecord::Refused { message, .. }) => {
                bail!("Launch {} was already refused: {message}", self.id)
            }
            Some(LaunchRecord::Admitted(_)) => {
                bail!("Launch {} already ran; nothing was started again.", self.id)
            }
        }
    }

    /// Takes this launch's own lock, held until its decision is published,
    /// then checks again that it is undecided: two executors of one launch id
    /// never both reach a replacement, and the one that waited finds the
    /// other's decision and stops nothing.
    pub fn lock(&self, store: &RunningServiceStore) -> Result<LaunchLock> {
        let lock = store.launches().lock_launch(&self.id)?;
        self.ensure_undecided(store)?;
        Ok(lock)
    }

    /// The checks under the lock, in the contract's order: the expiry, the
    /// revision, then for a remote launch the profile.
    pub fn check(&self, loaded: &Loaded<'_>) -> Result<(), Refusal> {
        if let Some(expires_at) = self.expires_at
            && unix_ms_now() >= expires_at
        {
            return Err(Refusal {
                code: RefusalCode::Expired,
                error: anyhow!(
                    "This launch expired before it could start (launch {}); nothing was started.",
                    self.id
                ),
            });
        }
        if let Some(expected) = &self.expect_rev {
            let revision = (loaded.revision)().map_err(Refusal::conflict)?;
            if revision != *expected {
                return Err(Refusal::conflict(anyhow!(
                    "The service changed since this launch was checked (revision {revision}, expected {expected}); nothing was started."
                )));
            }
        }
        if self.remote && !allowed_remotely(loaded.profiles, loaded.profile) {
            return Err(Refusal {
                code: RefusalCode::RefusedProfile,
                error: anyhow!(
                    "Profile '{}' cannot be started from another machine: it is dangerous, unknown or removed. Nothing was started.",
                    loaded.profile.unwrap_or("default")
                ),
            });
        }
        Ok(())
    }

    /// Publishes the refusal, and returns the error `localify dev` exits with.
    /// A decision already there, such as the reconciler's `expired`, stands.
    pub fn refuse(&self, store: &RunningServiceStore, refusal: Refusal) -> Error {
        let record = LaunchRecord::Refused {
            code: refusal.code,
            message: format!("{:#}", refusal.error),
            at: unix_ms_now(),
        };
        match store.launches().publish(&self.id, &record) {
            Ok(_) => refusal.error,
            Err(error) => refusal
                .error
                .context(format!("and its refusal could not be recorded: {error:#}")),
        }
    }

    /// Publishes the admission. Another decision found there wins: a launch
    /// the reconciler declared expired, while this one was between its checks
    /// and here, never starts.
    pub fn admit(&self, store: &RunningServiceStore, service: &ServiceKey) -> Result<()> {
        pause_at("launch-admission");
        let pid = std::process::id();
        let me = process::process_info(pid)
            .ok_or_else(|| anyhow!("Failed to read this process's identity from the kernel"))?;
        let boot_id = process::boot_id()
            .ok_or_else(|| anyhow!("Failed to read this boot's id from the kernel"))?;
        let admitted = LaunchRecord::Admitted(Box::new(AdmittedLaunch {
            run_id: self.id.clone(),
            service: service.clone(),
            executor: ExecutorIdentity {
                pid,
                start: me.start,
                sid: (me.sid == Some(pid)).then_some(pid),
                boot_id,
            },
            requested_by: self.requested_by.clone(),
            at: unix_ms_now(),
            exit: None,
        }));
        let launches = store.launches();
        match launches.publish(&self.id, &admitted)? {
            Publication::Published => {}
            Publication::Decided(LaunchRecord::Expired { .. }) => bail!(
                "This launch expired before it could start (launch {}); nothing was started.",
                self.id
            ),
            Publication::Decided(_) => bail!(
                "Launch {} was already decided; nothing was started.",
                self.id
            ),
        }
        // Later launches remove what is past its retention.
        let _ = store.prune_launches();
        pause_at("launch-admitted");
        Ok(())
    }

    /// Gets the run's folder ready before any profile helper runs, and has
    /// each helper this executor starts witnessed there (see
    /// [`record_alive`]): should it die before its run record, the helpers
    /// and what they started are proven this launch's, and ended.
    pub fn witness_helpers(&self, store: &RunningServiceStore) -> Result<()> {
        let dir = store.run_dir(&self.id);
        output::create_run_dir(&dir)?;
        helpers::helpers().witness(move || {
            let _ = record_alive(&dir);
        });
        Ok(())
    }

    /// Records that the admitted launch ended before its service started,
    /// and removes the run's folder unless the app made it for its terminal.
    pub fn exit_before_start(
        &self,
        store: &RunningServiceStore,
        reason: ExitReason,
        origin: ServiceOrigin,
    ) {
        let _ = store
            .launches()
            .add_exit(&self.id, LaunchExit::now(None, None, Some(reason)));
        if origin != ServiceOrigin::App {
            let _ = std::fs::remove_dir_all(store.run_dir(&self.id));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";

    fn remote() -> DispatchFlags {
        DispatchFlags {
            remote_launch: Some(ID.into()),
            launch_expires: Some(unix_ms_now() + 15_000),
            expect_rev: Some("0123456789abcdef".into()),
            request: Some("machine-a:req-1".into()),
            ..DispatchFlags::default()
        }
    }

    fn refusal(dispatch: &Dispatch, loaded: &Loaded<'_>) -> Option<RefusalCode> {
        dispatch.check(loaded).err().map(|refusal| refusal.code)
    }

    #[test]
    fn a_person_s_own_launch_has_nothing_to_fence() {
        assert!(DispatchFlags::default().parse().unwrap().is_none());
        let stray = DispatchFlags {
            expect_rev: Some("0123456789abcdef".into()),
            ..DispatchFlags::default()
        };
        assert!(stray.parse().is_err());
    }

    #[test]
    fn a_remote_launch_comes_with_its_expiry_revision_and_request() {
        let dispatch = remote().parse().unwrap().unwrap();
        assert!(dispatch.is_remote());
        assert_eq!(dispatch.id.as_str(), ID);

        for missing in [
            DispatchFlags {
                launch_expires: None,
                ..remote()
            },
            DispatchFlags {
                expect_rev: None,
                ..remote()
            },
            DispatchFlags {
                request: None,
                ..remote()
            },
            DispatchFlags {
                launch_id: Some(ID.into()),
                ..remote()
            },
        ] {
            assert!(missing.clone().parse().is_err(), "{missing:?}");
        }
        let local_request = DispatchFlags {
            launch_id: Some(ID.into()),
            request: Some("machine-a:req-1".into()),
            ..DispatchFlags::default()
        };
        assert!(local_request.parse().is_err());
    }

    #[test]
    fn checks_refuse_in_order_expiry_revision_then_profile() {
        let checkout = tempfile::tempdir().unwrap();
        fs::write(
            checkout.path().join("localify.toml"),
            "[profiles.default]\nsources = []\n\n[profiles.prod]\ndanger = true\nsources = []\n",
        )
        .unwrap();
        let profiles = LaunchProfiles::load(checkout.path());
        let same = || Ok("0123456789abcdef".to_string());
        let changed = || Ok("fedcba9876543210".to_string());
        let loaded = |profile: Option<&'static str>, revision: &'static str| Loaded {
            profiles: &profiles,
            profile,
            revision: if revision == "same" { &same } else { &changed },
        };
        let dispatch = remote().parse().unwrap().unwrap();
        let expired = DispatchFlags {
            launch_expires: Some(unix_ms_now() - 1),
            ..remote()
        }
        .parse()
        .unwrap()
        .unwrap();

        assert_eq!(refusal(&dispatch, &loaded(None, "same")), None);
        assert_eq!(refusal(&dispatch, &loaded(Some("default"), "same")), None);
        assert_eq!(
            refusal(&expired, &loaded(Some("prod"), "changed")),
            Some(RefusalCode::Expired)
        );
        assert_eq!(
            refusal(&dispatch, &loaded(Some("prod"), "changed")),
            Some(RefusalCode::Conflict)
        );
        assert_eq!(
            refusal(&dispatch, &loaded(Some("prod"), "same")),
            Some(RefusalCode::RefusedProfile)
        );
        assert_eq!(
            refusal(&dispatch, &loaded(Some("removed"), "same")),
            Some(RefusalCode::RefusedProfile)
        );
    }

    #[test]
    fn a_local_launch_may_use_a_dangerous_profile() {
        let checkout = tempfile::tempdir().unwrap();
        fs::write(
            checkout.path().join("localify.toml"),
            "[profiles.prod]\ndanger = true\nsources = []\n",
        )
        .unwrap();
        let profiles = LaunchProfiles::load(checkout.path());
        let local = DispatchFlags {
            launch_id: Some(ID.into()),
            ..DispatchFlags::default()
        }
        .parse()
        .unwrap()
        .unwrap();

        let loaded = Loaded {
            profiles: &profiles,
            profile: Some("prod"),
            revision: &|| unreachable!("no revision is expected"),
        };
        assert_eq!(refusal(&local, &loaded), None);
    }
}
