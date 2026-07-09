//! Test helpers

#[cfg(feature = "wire")]
pub mod wire;

/// Test helper to create a [`BTreeMap`] with keys and values that support [`FromStr`]
///
/// # Examples
/// ```
/// use std::collections::BTreeMap;
/// use std::net::IpAddr;
/// use std::path::PathBuf;
/// use nix_compat::btree_map;
///
/// let mut m : BTreeMap<IpAddr, PathBuf> = BTreeMap::new();
/// m.insert("127.0.0.1".parse().unwrap(), "/nix/store".parse().unwrap());
/// m.insert("192.168.1.1".parse().unwrap(), "/nix/var".parse().unwrap());
///
/// let m2 : BTreeMap<IpAddr, PathBuf> = btree_map![
///     "127.0.0.1" => "/nix/store",
///     "192.168.1.1" => "/nix/var",
/// ];
/// assert_eq!(m, m2);
/// ```
///
/// [`BTreeMap`]: std::collections::BTreeMap
/// [`FromStr`]: std::str::FromStr
#[macro_export]
macro_rules! btree_map {
    () => { BTreeMap::new() };
    ($($k:expr => $v:expr),+ $(,)?) => {{
        let mut ret = std::collections::BTreeMap::new();
        $(
            ret.insert($k.parse().unwrap(), $v.parse().unwrap());
        )+
        ret
    }};
}

/// Test helper to create a [`BTreeSet`] with values that support [`FromStr`]
///
/// # Examples
/// ```
/// use std::collections::BTreeSet;
/// use std::net::IpAddr;
/// use nix_compat::btree_set;
///
/// let mut m : BTreeSet<IpAddr> = BTreeSet::new();
/// m.insert("127.0.0.1".parse().unwrap());
/// m.insert("192.168.1.1".parse().unwrap());
///
/// let m2 : BTreeSet<IpAddr> = btree_set![
///     "127.0.0.1",
///     "192.168.1.1",
/// ];
/// assert_eq!(m, m2);
/// ```
///
/// [`BTreeSet`]: std::collections::BTreeSet
/// [`FromStr`]: std::str::FromStr
#[macro_export]
macro_rules! btree_set {
    () => { BTreeSet::new() };
    ($($v:expr),+ $(,)?) => {{
        let mut ret = std::collections::BTreeSet::new();
        $(
            ret.insert($v.parse().unwrap());
        )+
        ret
    }};
}
