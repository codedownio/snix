use smol_str::SmolStr;
use std::fmt;

use crate::{
    nixbase32,
    store_path::{
        DIGEST_SIZE, ENCODED_DIGEST_SIZE, ParseStorePathError, STORE_DIR_WITH_SLASH, StorePath,
        validate_name,
    },
};

/// Like [StorePath], but with a `&str` for the name.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct StorePathRef<'a> {
    digest: [u8; DIGEST_SIZE],
    name: &'a str,
}

impl<'a> StorePathRef<'a> {
    pub fn digest(&self) -> &[u8; DIGEST_SIZE] {
        &self.digest
    }

    pub fn name(&self) -> &'a str {
        self.name
    }

    pub fn to_owned(&self) -> StorePath {
        StorePath {
            digest: self.digest,
            name: SmolStr::new(self.name),
        }
    }

    /// Construct by passing the `$digest-$name` string that comes after
    /// [STORE_DIR_WITH_SLASH].
    pub fn from_bytes(s: &'a [u8]) -> Result<Self, ParseStorePathError> {
        // the whole string needs to be at least:
        //
        // - 32 characters (encoded hash)
        // - 1 dash
        // - 1 character for the name
        if s.len() < ENCODED_DIGEST_SIZE + 2 {
            Err(ParseStorePathError::Length)?
        }

        let digest = nixbase32::decode_fixed(&s[..ENCODED_DIGEST_SIZE])?;

        if s[ENCODED_DIGEST_SIZE] != b'-' {
            return Err(ParseStorePathError::MissingDash);
        }

        Ok(Self {
            digest,
            name: validate_name(&s[ENCODED_DIGEST_SIZE + 1..])?,
        })
    }

    /// Construct from a name and digest.
    /// The name is validated, and the digest checked for size.
    pub fn from_name_and_digest(name: &'a str, digest: &[u8]) -> Result<Self, ParseStorePathError> {
        let digest_fixed = digest.try_into().map_err(|_| ParseStorePathError::Length)?;
        Self::from_name_and_digest_fixed(name, digest_fixed)
    }

    /// Construct from a name and digest of correct length.
    /// The name is validated.
    pub fn from_name_and_digest_fixed(
        name: &'a str,
        digest: [u8; DIGEST_SIZE],
    ) -> Result<Self, ParseStorePathError> {
        Ok(Self {
            name: validate_name(name.as_bytes())?,
            digest,
        })
    }

    /// Construct from an absolute store path string.
    /// This is equivalent to calling [StorePathRef::from_bytes], but stripping
    /// the [STORE_DIR_WITH_SLASH] prefix before.
    pub fn from_absolute_path(s: &'a [u8]) -> Result<Self, ParseStorePathError> {
        match s.strip_prefix(STORE_DIR_WITH_SLASH.as_bytes()) {
            Some(s_stripped) => Self::from_bytes(s_stripped),
            None => Err(ParseStorePathError::MissingStoreDir),
        }
    }

    /// Decompose a string into a [StorePathRef] and a [std::path::Path]
    /// containing the rest of the path, or an error.
    pub fn from_absolute_path_full<'p, P>(
        path: &'p P,
    ) -> Result<(Self, &'p std::path::Path), ParseStorePathError>
    where
        P: AsRef<std::path::Path> + 'p + ?Sized,
        'p: 'a,
    {
        // strip [STORE_DIR_WITH_SLASH] from path
        let p = path
            .as_ref()
            .strip_prefix(STORE_DIR_WITH_SLASH)
            .map_err(|_| ParseStorePathError::MissingStoreDir)?;

        let mut components = p.components();

        use bstr::ByteSlice;
        let first_component = <[u8]>::from_os_str(
            components
                .next()
                .ok_or(ParseStorePathError::Length)?
                .as_os_str(),
        )
        .ok_or(ParseStorePathError::Name)?;

        // The first component must be parse-able as a [StorePath].
        if first_component.len() < 34 {
            return Err(ParseStorePathError::Length);
        }

        let store_path = Self::from_bytes(first_component)?;

        Ok((store_path, components.as_path()))
    }

    /// Returns as an absolute store path (prefixed with [STORE_DIR_WITH_SLASH]).
    pub fn to_absolute_path(&self) -> String {
        self.as_absolute_path_fmt().to_string()
    }

    /// Returns a formatter writing an absolute store path (prefixed with [STORE_DIR_WITH_SLASH]).
    pub fn as_absolute_path_fmt(&'a self) -> impl std::fmt::Display + 'a {
        struct WithAbsolutePath<'a>(&'a StorePathRef<'a>);

        impl std::fmt::Display for WithAbsolutePath<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{STORE_DIR_WITH_SLASH}{}", self.0)
            }
        }
        WithAbsolutePath(self)
    }
}

impl fmt::Display for StorePathRef<'_> {
    /// The string representation of a store path starts with a digest (20
    /// bytes), [crate::nixbase32]-encoded, followed by a `-`,
    /// and ends with the name.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", nixbase32::encode(&self.digest), self.name)
    }
}

impl PartialOrd for StorePathRef<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> From<StorePathRef<'a>> for StorePath {
    fn from(value: StorePathRef<'a>) -> Self {
        Self {
            digest: value.digest,
            name: SmolStr::new(value.name),
        }
    }
}

impl StorePath {
    pub fn as_ref(&self) -> StorePathRef<'_> {
        StorePathRef {
            digest: self.digest,
            name: &self.name,
        }
    }
}

/// `StorePath`s are sorted by their reverse digest to match the sorting order
/// of the nixbase32-encoded string.
impl Ord for StorePathRef<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.digest.iter().rev().cmp(other.digest.iter().rev())
    }
}

// Ensures it's possible to get() from a HashMap using StorePathRef<'_>,
// even if it itself uses StorePath<String>
#[cfg(feature = "hashbrown")]
impl hashbrown::Equivalent<StorePath> for StorePathRef<'_> {
    fn equivalent(&self, key: &StorePath) -> bool {
        self.digest() == key.digest() && self.name() == key.name()
    }
}

#[cfg(feature = "serde")]
impl<'a, 'de: 'a> serde::Deserialize<'de> for StorePathRef<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let string: &'de str = serde::Deserialize::deserialize(deserializer)?;
        let stripped: Option<&str> = string.strip_prefix(STORE_DIR_WITH_SLASH);
        let stripped: &str = stripped.ok_or_else(|| {
            serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(string),
                &"store path prefix",
            )
        })?;
        StorePathRef::from_bytes(stripped.as_bytes()).map_err(|_| {
            serde::de::Error::invalid_value(serde::de::Unexpected::Str(string), &"StorePath")
        })
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for StorePathRef<'_> {
    fn serialize<SR>(&self, serializer: SR) -> Result<SR::Ok, SR::Error>
    where
        SR: serde::Serializer,
    {
        let string: String = self.to_absolute_path();
        string.serialize(serializer)
    }
}
