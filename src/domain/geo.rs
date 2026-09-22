//! A geographic position submitted with an attendance record.

use super::error::DomainError;

/// A latitude/longitude pair, in decimal degrees.
///
/// Always finite: the fields are private and [`Coordinates::new`] rejects
/// non-finite components, so holding one of these means the position is
/// submittable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coordinates {
    latitude: f64,
    longitude: f64,
}

impl Coordinates {
    /// The origin, submitted when no coordinates were configured.
    pub const ORIGIN: Self = Self {
        latitude: 0.0,
        longitude: 0.0,
    };

    /// Build a position, rejecting any component that is not a finite number.
    ///
    /// # Errors
    /// Returns [`DomainError::InvalidCoordinates`] when either component is
    /// infinite or `NaN`, so a bogus position cannot reach the server.
    pub const fn new(latitude: f64, longitude: f64) -> Result<Self, DomainError> {
        if !latitude.is_finite() || !longitude.is_finite() {
            return Err(DomainError::InvalidCoordinates {
                reason: "latitude and longitude must be finite numbers",
            });
        }
        Ok(Self {
            latitude,
            longitude,
        })
    }

    /// Latitude in decimal degrees.
    pub const fn latitude(&self) -> f64 {
        self.latitude
    }

    /// Longitude in decimal degrees.
    pub const fn longitude(&self) -> f64 {
        self.longitude
    }
}

#[cfg(test)]
mod tests {
    use super::Coordinates;

    #[test]
    fn accepts_a_finite_pair() {
        // Given a real position.
        let point = Coordinates::new(22.3364, 114.1796).expect("a finite pair is valid");

        // When read back.
        // Then each component is preserved in the order given.
        assert!((point.latitude() - 22.3364).abs() < f64::EPSILON);
        assert!((point.longitude() - 114.1796).abs() < f64::EPSILON);
    }

    #[test]
    fn rejects_a_non_finite_component() {
        // Given pairs with an infinite or `NaN` component.
        // When constructed.
        // Then both are refused rather than submitted as a bogus position.
        assert!(Coordinates::new(f64::INFINITY, 0.0).is_err());
        assert!(Coordinates::new(0.0, f64::NAN).is_err());
    }

    #[test]
    fn origin_is_the_zero_pair() {
        // Given the default position.
        // When read.
        // Then it is the origin, which the submission URL encodes as 0,0.
        assert!(Coordinates::ORIGIN.latitude().abs() < f64::EPSILON);
        assert!(Coordinates::ORIGIN.longitude().abs() < f64::EPSILON);
    }
}
