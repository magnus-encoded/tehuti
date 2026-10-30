//! Validated, pre-registered continuous value ranges.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Registration")]
pub struct Adjuster {
    name: String,
    minimum: i32,
    maximum: i32,
    value: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    name: String,
    minimum: i32,
    maximum: i32,
    value: i32,
}

impl TryFrom<Registration> for Adjuster {
    type Error = String;
    fn try_from(registration: Registration) -> Result<Self, String> {
        let Registration {
            name,
            minimum,
            maximum,
            value,
        } = registration;
        let name = crate::normalize_phrase(&name);
        if name.is_empty()
            || name.chars().count() > 24
            || name.chars().any(char::is_control)
            || name == "volume"
        {
            return Err("adjuster name must be nonempty, at most 24 characters and not reserved".into());
        }
        if minimum >= maximum || !(minimum..=maximum).contains(&value) {
            return Err("adjuster needs an ordered range containing its value".into());
        }
        Ok(Self {
            name,
            minimum,
            maximum,
            value,
        })
    }
}

impl Adjuster {
    pub fn set_value(&mut self, value: i32) {
        self.value = value.clamp(self.minimum, self.maximum);
    }
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[must_use]
    pub const fn minimum(&self) -> i32 {
        self.minimum
    }
    #[must_use]
    pub const fn maximum(&self) -> i32 {
        self.maximum
    }
    #[must_use]
    pub const fn value(&self) -> i32 {
        self.value
    }
}
