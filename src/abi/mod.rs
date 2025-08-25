//! Common ABI implementation.

use std::hash::{BuildHasher, Hash};
use std::str::FromStr;

pub use tycho_types_abi_proc::{FromAbi, IntoAbi, WithAbiType};

pub use self::contract::{
    Contract, Event, EventBuilder, ExternalInput, Function, FunctionBuilder, UnsignedBody,
    UnsignedExternalMessage,
};
pub use self::signature::{extend_signature_with_id, sign_with_signature_id};
pub use self::traits::{
    FromAbi, FromAbiIter, FromPlainAbi, IgnoreName, IntoAbi, IntoPlainAbi, WithAbiType,
    WithPlainAbiType,
};
pub use self::ty::{
    AbiHeaderType, AbiType, AbiTypeFlatten, NamedAbiType, NamedAbiTypeFlatten, PlainAbiType,
};
pub use self::value::{AbiHeader, AbiValue, NamedAbiValue, PlainAbiValue};

pub mod error;

mod contract;
mod signature;
mod traits;
mod ty;
mod value;

#[cfg(test)]
mod tests;

#[doc(hidden)]
pub mod __export {
    pub use anyhow;
}

/// ABI version.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct AbiVersion {
    /// Major version component.
    pub major: u8,
    /// Minor version component.
    pub minor: u8,
}

impl AbiVersion {
    /// A legacy ABI version.
    pub const V1_0: Self = Self::new(1, 0);
    /// A base version of an ABI 2.
    pub const V2_0: Self = Self::new(2, 0);
    /// A base version with strings and refs.
    pub const V2_1: Self = Self::new(2, 1);
    /// Same as 2.1 but with a more compact address serialization.
    pub const V2_2: Self = Self::new(2, 2);
    /// Same as 2.2 but uses an address during signing.
    pub const V2_3: Self = Self::new(2, 3);
    /// Same as 2.3 but init_data is replaced with init fields in abi
    pub const V2_4: Self = Self::new(2, 4);

    /// Creates an ABI version from components.
    pub const fn new(major: u8, minor: u8) -> Self {
        Self { major, minor }
    }
}

impl FromStr for AbiVersion {
    type Err = error::ParseAbiVersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (major, minor) = ok!(s
            .split_once('.')
            .ok_or(error::ParseAbiVersionError::InvalidFormat));

        Ok(Self {
            major: ok!(major
                .parse()
                .map_err(error::ParseAbiVersionError::InvalidComponent)),
            minor: ok!(minor
                .parse()
                .map_err(error::ParseAbiVersionError::InvalidComponent)),
        })
    }
}

impl std::fmt::Display for AbiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// A wrapper around [`AbiType`], [`NamedAbiType`], [`AbiValue`] and [`NamedAbiValue`]
/// that implements hash/comparison traits without name.
#[repr(transparent)]
pub struct WithoutName<T>(pub T);

impl<T> WithoutName<T> {
    /// Wraps a reference of the inner type.
    pub fn wrap(value: &T) -> &Self {
        // SAFETY: HashWithoutName<T> is #[repr(transparent)]
        unsafe { &*(value as *const T as *const Self) }
    }

    /// Wraps a slice of the inner type.
    pub fn wrap_slice(value: &[T]) -> &[Self] {
        // SAFETY: HashWithoutName<T> is #[repr(transparent)]
        unsafe { &*(value as *const [T] as *const [Self]) }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for WithoutName<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WithoutName").field(&self.0).finish()
    }
}

impl<T: Clone> Clone for WithoutName<T> {
    #[inline]
    fn clone(&self) -> Self {
        WithoutName(self.0.clone())
    }
}

impl<T> Eq for WithoutName<T> where WithoutName<T>: PartialEq {}

impl<T> PartialOrd for WithoutName<T>
where
    WithoutName<T>: Ord,
{
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> PartialEq for WithoutName<Vec<T>>
where
    WithoutName<T>: PartialEq,
{
    fn eq(&self, WithoutName(other): &Self) -> bool {
        WithoutName::wrap_slice(self.0.as_slice()) == WithoutName::wrap_slice(other.as_slice())
    }
}

impl<K, V> PartialEq for WithoutName<std::collections::BTreeMap<K, V>>
where
    K: PartialEq,
    WithoutName<V>: PartialEq,
{
    fn eq(&self, WithoutName(other): &Self) -> bool {
        self.0.len() == other.len()
            && self.0.iter().zip(other).all(|((ak, av), (bk, bv))| {
                (ak, WithoutName::wrap(av)) == (bk, WithoutName::wrap(bv))
            })
    }
}

impl<K, V, S> PartialEq for WithoutName<std::collections::HashMap<K, V, S>>
where
    K: Eq + Hash,
    WithoutName<V>: PartialEq,
    S: BuildHasher,
{
    fn eq(&self, WithoutName(other): &Self) -> bool {
        if self.0.len() != other.len() {
            return false;
        }

        self.0.iter().all(|(key, value)| {
            other
                .get(key)
                .is_some_and(|v| WithoutName::wrap(value) == WithoutName::wrap(v))
        })
    }
}
