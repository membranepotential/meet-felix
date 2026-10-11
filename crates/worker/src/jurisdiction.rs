//! Where new Durable Objects run and store data
//! (<https://developers.cloudflare.com/durable-objects/reference/data-location/>): the var
//! [`ROOM_JURISDICTION`] pins every new room and recorder to one jurisdiction (D5 in
//! `docs/architecture`).

use std::fmt;
use std::str::FromStr;

use worker::{ObjectId, ObjectNamespace, Result};

use crate::ROOM_JURISDICTION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jurisdiction {
    Eu,
    Us,
    Fedramp,
}

/// A [`ROOM_JURISDICTION`] value that names no jurisdiction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownJurisdiction(String);

impl fmt::Display for UnknownJurisdiction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{ROOM_JURISDICTION} must be empty, \"eu\", \"us\" or \"fedramp\", not {:?}",
            self.0
        )
    }
}

impl std::error::Error for UnknownJurisdiction {}

impl From<UnknownJurisdiction> for worker::Error {
    fn from(e: UnknownJurisdiction) -> Self {
        Self::RustError(e.to_string())
    }
}

impl FromStr for Jurisdiction {
    type Err = UnknownJurisdiction;

    fn from_str(name: &str) -> std::result::Result<Self, Self::Err> {
        match name {
            "eu" => Ok(Self::Eu),
            "us" => Ok(Self::Us),
            "fedramp" => Ok(Self::Fedramp),
            other => Err(UnknownJurisdiction(other.to_owned())),
        }
    }
}

impl Jurisdiction {
    /// Reads a [`ROOM_JURISDICTION`] value: empty means none (local workerd implements none),
    /// any other value must name a jurisdiction.
    pub fn parse_var(value: &str) -> std::result::Result<Option<Self>, UnknownJurisdiction> {
        match value {
            "" => Ok(None),
            name => name.parse().map(Some),
        }
    }

    /// [`Self::parse_var`] with the error as text. Only `room.rs` calls it; it goes once
    /// `RoomObject::new_recorder` calls [`Self::parse_var`] and [`unique_id`].
    pub fn from_var(value: &str) -> std::result::Result<Option<Self>, String> {
        Self::parse_var(value).map_err(|e| e.to_string())
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eu => "eu",
            Self::Us => "us",
            Self::Fedramp => "fedramp",
        }
    }
}

/// A new object id in `namespace`, in `jurisdiction` if there is one.
pub fn unique_id(
    namespace: &ObjectNamespace,
    jurisdiction: Option<Jurisdiction>,
) -> Result<ObjectId<'_>> {
    match jurisdiction {
        None => namespace.unique_id(),
        Some(jurisdiction) => namespace.unique_id_with_jurisdiction(jurisdiction.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_var_accepts_known_names_or_nothing() {
        assert_eq!(Jurisdiction::parse_var(""), Ok(None));
        assert_eq!(Jurisdiction::parse_var("eu"), Ok(Some(Jurisdiction::Eu)));
        for j in [Jurisdiction::Eu, Jurisdiction::Us, Jurisdiction::Fedramp] {
            assert_eq!(j.as_str().parse(), Ok(j));
        }
        for bad in ["EU", "de", " eu"] {
            assert_eq!(
                Jurisdiction::parse_var(bad),
                Err(UnknownJurisdiction(bad.into()))
            );
        }
        assert_eq!(
            Jurisdiction::from_var("de"),
            Err(format!(
                "{ROOM_JURISDICTION} must be empty, \"eu\", \"us\" or \"fedramp\", not \"de\""
            ))
        );
    }
}
