/// How many units of a Localify Cloud add-on one checkout buys: serving
/// machines, or packs of 10 public URLs. Localify Cloud counts at most 20 of
/// each, so it sells at most 20 at once (`docs/public-urls.md`, "Plans").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddonUnits(u8);

impl AddonUnits {
    pub const MAX: u8 = 20;

    pub fn new(units: u32) -> Result<Self, AddonUnitsError> {
        match u8::try_from(units) {
            Ok(units) if (1..=Self::MAX).contains(&units) => Ok(Self(units)),
            _ => Err(AddonUnitsError(units)),
        }
    }

    pub fn value(self) -> u8 {
        self.0
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Add-ons are bought 1 to 20 at a time, got: {0}")]
pub struct AddonUnitsError(pub u32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_go_from_one_to_twenty() {
        assert_eq!(AddonUnits::new(1).unwrap().value(), 1);
        assert_eq!(AddonUnits::new(20).unwrap().value(), 20);
        assert!(AddonUnits::new(0).is_err());
        assert!(AddonUnits::new(21).is_err());
        assert!(AddonUnits::new(256).is_err());
    }
}
