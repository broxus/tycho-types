use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hash};
use std::num::NonZeroU8;
use std::rc::Rc;
use std::sync::Arc;

use super::{
    AbiType, AbiValue, NamedAbiType, NamedAbiValue, PlainAbiType, PlainAbiValue, WithoutName,
};
use crate::cell::{Cell, HashBytes, Lazy};
use crate::models::message::{IntAddr, StdAddr, VarAddr};
use crate::num::*;
use anyhow::Result;
use bytes::Bytes;
use everscale_types::models::{AnyAddr, ExtAddr};
use num_bigint::{BigInt, BigUint};
use num_traits::ToPrimitive;

/// ABI entity wrapper.
pub trait IgnoreName {
    /// Wrapped ABI entity.
    type Unnamed<'a>
    where
        Self: 'a;

    /// Wraps an ABI entity into [`WithoutName`].
    fn ignore_name(&self) -> Self::Unnamed<'_>;
}

impl<T: IgnoreName> IgnoreName for &'_ T {
    type Unnamed<'a>
        = T::Unnamed<'a>
    where
        Self: 'a;

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        T::ignore_name(self)
    }
}

impl<T> IgnoreName for Vec<T>
where
    [T]: IgnoreName,
{
    type Unnamed<'a>
        = <[T] as IgnoreName>::Unnamed<'a>
    where
        Self: 'a;

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        <[T] as IgnoreName>::ignore_name(self.as_slice())
    }
}

impl<T: IgnoreName> IgnoreName for Box<T> {
    type Unnamed<'a>
        = T::Unnamed<'a>
    where
        Self: 'a;

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        T::ignore_name(self.as_ref())
    }
}

impl<T: IgnoreName> IgnoreName for Arc<T> {
    type Unnamed<'a>
        = T::Unnamed<'a>
    where
        Self: 'a;

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        T::ignore_name(self.as_ref())
    }
}

impl<T: IgnoreName> IgnoreName for Rc<T> {
    type Unnamed<'a>
        = T::Unnamed<'a>
    where
        Self: 'a;

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        T::ignore_name(self.as_ref())
    }
}

impl<T: IgnoreName> IgnoreName for Option<T> {
    type Unnamed<'a>
        = Option<T::Unnamed<'a>>
    where
        Self: 'a;

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        self.as_ref().map(|t| T::ignore_name(t))
    }
}

impl IgnoreName for AbiType {
    type Unnamed<'a> = &'a WithoutName<AbiType>;

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        WithoutName::wrap(self)
    }
}

impl IgnoreName for [AbiType] {
    type Unnamed<'a> = &'a [WithoutName<AbiType>];

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        WithoutName::wrap_slice(self)
    }
}

impl IgnoreName for NamedAbiType {
    type Unnamed<'a> = &'a WithoutName<NamedAbiType>;

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        WithoutName::wrap(self)
    }
}

impl IgnoreName for [NamedAbiType] {
    type Unnamed<'a> = &'a [WithoutName<NamedAbiType>];

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        WithoutName::wrap_slice(self)
    }
}

impl IgnoreName for AbiValue {
    type Unnamed<'a> = &'a WithoutName<AbiValue>;

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        WithoutName::wrap(self)
    }
}

impl IgnoreName for [AbiValue] {
    type Unnamed<'a> = &'a [WithoutName<AbiValue>];

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        WithoutName::wrap_slice(self)
    }
}

impl IgnoreName for NamedAbiValue {
    type Unnamed<'a> = &'a WithoutName<NamedAbiValue>;

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        WithoutName::wrap(self)
    }
}

impl IgnoreName for [NamedAbiValue] {
    type Unnamed<'a> = &'a [WithoutName<NamedAbiValue>];

    #[inline]
    fn ignore_name(&self) -> Self::Unnamed<'_> {
        WithoutName::wrap_slice(self)
    }
}

/// A type with a known ABI type.
pub trait WithAbiType {
    /// Returns a corresponding ABI type.
    fn abi_type() -> AbiType;
}

macro_rules! impl_with_abi_type {
    ($($ty:ty => $ident:ident$(($n:expr))?),*$(,)?) => {$(
        impl WithAbiType for $ty {
            fn abi_type() -> AbiType {
                AbiType::$ident$(($n))?
            }
        }
    )*};
}

impl_with_abi_type! {
    u8 => Uint(8),
    u16 => Uint(16),
    u32 => Uint(32),
    u64 => Uint(64),
    u128 => Uint(128),
    HashBytes => Uint(256),
    SplitDepth => Uint(5),
    Uint9 => Uint(9),
    Uint12 => Uint(12),
    Uint15 => Uint(15),

    i8 => Int(8),
    i16 => Int(16),
    i32 => Int(32),
    i64 => Int(64),
    i128 => Int(128),

    bool => Bool,

    VarUint24 => VarUint(NonZeroU8::new(4).unwrap()),
    VarUint56 => VarUint(NonZeroU8::new(8).unwrap()),
    VarUint248 => VarUint(NonZeroU8::new(32).unwrap()),

    Tokens => Token,

    Cell => Cell,

    Bytes => Bytes,
    String => String,
    str => String,

    AnyAddr => Address,
    ExtAddr => Address,

    IntAddr => Address,
    StdAddr => Address,
    VarAddr => Address,
}

impl<T: WithAbiType> WithAbiType for &T {
    fn abi_type() -> AbiType {
        T::abi_type()
    }
}

impl<T: WithAbiType> WithAbiType for &mut T {
    fn abi_type() -> AbiType {
        T::abi_type()
    }
}

impl<T: WithAbiType> WithAbiType for Box<T> {
    fn abi_type() -> AbiType {
        T::abi_type()
    }
}

impl<T: WithAbiType> WithAbiType for Arc<T> {
    fn abi_type() -> AbiType {
        T::abi_type()
    }
}

impl<T: WithAbiType> WithAbiType for Rc<T> {
    fn abi_type() -> AbiType {
        T::abi_type()
    }
}

impl<T: WithAbiType> WithAbiType for Option<T> {
    fn abi_type() -> AbiType {
        AbiType::Optional(Arc::new(T::abi_type()))
    }
}

impl<T: WithAbiType> WithAbiType for Vec<T> {
    fn abi_type() -> AbiType {
        if typeid::of::<T>() == typeid::of::<u8>() {
            AbiType::Bytes
        } else {
            AbiType::Array(Arc::new(T::abi_type()))
        }
    }
}

impl<T: WithAbiType> WithAbiType for [T] {
    fn abi_type() -> AbiType {
        if typeid::of::<T>() == typeid::of::<u8>() {
            AbiType::Bytes
        } else {
            AbiType::Array(Arc::new(T::abi_type()))
        }
    }
}

impl<T: WithAbiType, const N: usize> WithAbiType for [T; N] {
    fn abi_type() -> AbiType {
        if typeid::of::<T>() == typeid::of::<u8>() {
            AbiType::FixedBytes(N)
        } else {
            AbiType::FixedArray(Arc::new(T::abi_type()), N)
        }
    }
}

impl<K: WithPlainAbiType, V: WithAbiType> WithAbiType for BTreeMap<K, V> {
    fn abi_type() -> AbiType {
        AbiType::Map(K::plain_abi_type(), Arc::new(V::abi_type()))
    }
}

impl<K: WithPlainAbiType, V: WithAbiType, S> WithAbiType for HashMap<K, V, S> {
    fn abi_type() -> AbiType {
        AbiType::Map(K::plain_abi_type(), Arc::new(V::abi_type()))
    }
}

#[cfg(feature = "models")]
impl<T: WithAbiType> WithAbiType for Lazy<T> {
    fn abi_type() -> AbiType {
        AbiType::Ref(Arc::new(T::abi_type()))
    }
}

impl WithAbiType for () {
    fn abi_type() -> AbiType {
        AbiType::Tuple(Arc::from([].as_slice()))
    }
}

macro_rules! impl_with_abi_type_for_tuple {
    ($($i:literal: $t:ident),+$(,)?) => {
        impl<$($t: WithAbiType),*> WithAbiType for ($($t),+,) {
            fn abi_type() -> AbiType {
                AbiType::Tuple(Arc::from(vec![
                    $(NamedAbiType::from_index($i, <$t as WithAbiType>::abi_type())),*
                ]))
            }
        }
    };
}

impl_with_abi_type_for_tuple! { 0: T0 }
impl_with_abi_type_for_tuple! { 0: T0, 1: T1 }
impl_with_abi_type_for_tuple! { 0: T0, 1: T1, 2: T2 }
impl_with_abi_type_for_tuple! { 0: T0, 1: T1, 2: T2, 3: T3 }
impl_with_abi_type_for_tuple! { 0: T0, 1: T1, 2: T2, 3: T3, 4: T4 }
impl_with_abi_type_for_tuple! { 0: T0, 1: T1, 2: T2, 3: T3, 4: T4, 5: T5 }
impl_with_abi_type_for_tuple! { 0: T0, 1: T1, 2: T2, 3: T3, 4: T4, 5: T5, 6: T6 }

/// A type with a known plain ABI type.
pub trait WithPlainAbiType: WithAbiType {
    /// Returns a corresponding plain ABI type.
    fn plain_abi_type() -> PlainAbiType;
}

macro_rules! impl_with_plain_abi_type {
    ($($ty:ty => $ident:ident$(($n:literal))?),*$(,)?) => {$(
        impl WithPlainAbiType for $ty {
            fn plain_abi_type() -> PlainAbiType {
                PlainAbiType::$ident$(($n))?
            }
        }
    )*};
}

impl_with_plain_abi_type! {
    u8 => Uint(8),
    u16 => Uint(16),
    u32 => Uint(32),
    u64 => Uint(64),
    u128 => Uint(128),
    HashBytes => Uint(256),
    SplitDepth => Uint(5),
    Uint9 => Uint(9),
    Uint12 => Uint(12),
    Uint15 => Uint(15),

    i8 => Int(8),
    i16 => Int(16),
    i32 => Int(32),
    i64 => Int(64),
    i128 => Int(128),

    bool => Bool,

    IntAddr => Address,
    StdAddr => Address,
    VarAddr => Address,
}

/// A type which can be converted into a plain ABI value.
pub trait IntoPlainAbi: IntoAbi {
    /// Returns a corresponding plain ABI value.
    ///
    /// NOTE: use [`IntoPlainAbi::into_plain_abi`] when building ABI from a temp value.
    fn as_plain_abi(&self) -> PlainAbiValue;

    /// Converts into a corresponding plain ABI value.
    fn into_plain_abi(self) -> PlainAbiValue
    where
        Self: Sized;
}

macro_rules! impl_into_plain_abi {
    ($($ty:ty => |$n:ident| { $expr1:expr, $expr2:expr $(,)? }),*$(,)?) => {$(
        impl IntoPlainAbi for $ty {
            #[inline]
            fn as_plain_abi(&self) -> PlainAbiValue {
                let $n = self;
                $expr1
            }

            #[inline]
            fn into_plain_abi(self) -> PlainAbiValue
            where
                Self: Sized
            {
                let $n = self;
                $expr2
            }
        }
    )*};
}

impl_into_plain_abi! {
    PlainAbiValue => |v| { v.clone(), v },

    u8 => |v| {
        PlainAbiValue::Uint(8, BigUint::from(*v)),
        PlainAbiValue::Uint(8, BigUint::from(v)),
    },
    u16 => |v| {
        PlainAbiValue::Uint(16, BigUint::from(*v)),
        PlainAbiValue::Uint(16, BigUint::from(v)),
    },
    u32 => |v| {
        PlainAbiValue::Uint(32, BigUint::from(*v)),
        PlainAbiValue::Uint(32, BigUint::from(v)),
    },
    u64 => |v| {
        PlainAbiValue::Uint(64, BigUint::from(*v)),
        PlainAbiValue::Uint(64, BigUint::from(v)),
    },
    u128 => |v| {
        PlainAbiValue::Uint(128, BigUint::from(*v)),
        PlainAbiValue::Uint(128, BigUint::from(v)),
    },
    HashBytes => |v| {
        PlainAbiValue::Uint(256, BigUint::from_bytes_be(v.as_array())),
        PlainAbiValue::Uint(256, BigUint::from_bytes_be(v.as_array())),
    },
    SplitDepth => |v| {
        PlainAbiValue::Uint(5, BigUint::from(v.into_bit_len())),
        PlainAbiValue::Uint(5, BigUint::from(v.into_bit_len())),
    },
    Uint9 => |v| {
        PlainAbiValue::Uint(9, BigUint::from(v.into_inner())),
        PlainAbiValue::Uint(9, BigUint::from(v.into_inner())),
    },
    Uint12 => |v| {
        PlainAbiValue::Uint(12, BigUint::from(v.into_inner())),
        PlainAbiValue::Uint(12, BigUint::from(v.into_inner())),
    },
    Uint15 => |v| {
        PlainAbiValue::Uint(15, BigUint::from(v.into_inner())),
        PlainAbiValue::Uint(15, BigUint::from(v.into_inner())),
    },

    i8 => |v| {
        PlainAbiValue::Int(8, BigInt::from(*v)),
        PlainAbiValue::Int(8, BigInt::from(v)),
    },
    i16 => |v| {
        PlainAbiValue::Int(16, BigInt::from(*v)),
        PlainAbiValue::Int(16, BigInt::from(v)),
    },
    i32 => |v| {
        PlainAbiValue::Int(32, BigInt::from(*v)),
        PlainAbiValue::Int(32, BigInt::from(v)),
    },
    i64 => |v| {
        PlainAbiValue::Int(64, BigInt::from(*v)),
        PlainAbiValue::Int(64, BigInt::from(v)),
    },
    i128 => |v| {
        PlainAbiValue::Int(128, BigInt::from(*v)),
        PlainAbiValue::Int(128, BigInt::from(v)),
    },

    bool => |v| {
        PlainAbiValue::Bool(*v),
        PlainAbiValue::Bool(v),
    },

    IntAddr => |v| {
        PlainAbiValue::Address(Box::new(v.clone())),
        PlainAbiValue::Address(Box::new(v)),
    },
    StdAddr => |v| {
        PlainAbiValue::Address(Box::new(v.clone().into())),
        PlainAbiValue::Address(Box::new(v.into())),
    },
    VarAddr => |v| {
        PlainAbiValue::Address(Box::new(v.clone().into())),
        PlainAbiValue::Address(Box::new(v.into())),
    },
}

/// A type which can be converted into an ABI value.
pub trait IntoAbi {
    /// Returns a corresponding ABI value.
    ///
    /// NOTE: use [`IntoAbi::into_abi`] when building ABI from a temp value.
    fn as_abi(&self) -> AbiValue;

    /// Converts into a corresponding ABI value.
    fn into_abi(self) -> AbiValue
    where
        Self: Sized;
}

macro_rules! impl_into_abi {
    ($($ty:ty => |$n:ident| { $expr1:expr, $expr2:expr $(,)? }),*$(,)?) => {$(
        impl IntoAbi for $ty {
            #[inline]
            fn as_abi(&self) -> AbiValue {
                let $n = self;
                $expr1
            }

            #[inline]
            fn into_abi(self) -> AbiValue
            where
                Self: Sized
            {
                let $n = self;
                $expr2
            }
        }
    )*};
}

impl_into_abi! {
    AbiValue => |v| { v.clone(), v },
    PlainAbiValue => |v| { v.clone().into(), v.into() },

    u8 => |v| {
        AbiValue::Uint(8, BigUint::from(*v)),
        AbiValue::Uint(8, BigUint::from(v)),
    },
    u16 => |v| {
        AbiValue::Uint(16, BigUint::from(*v)),
        AbiValue::Uint(16, BigUint::from(v)),
    },
    u32 => |v| {
        AbiValue::Uint(32, BigUint::from(*v)),
        AbiValue::Uint(32, BigUint::from(v)),
    },
    u64 => |v| {
        AbiValue::Uint(64, BigUint::from(*v)),
        AbiValue::Uint(64, BigUint::from(v)),
    },
    u128 => |v| {
        AbiValue::Uint(128, BigUint::from(*v)),
        AbiValue::Uint(128, BigUint::from(v)),
    },
    HashBytes => |v| {
        AbiValue::Uint(256, BigUint::from_bytes_be(v.as_array())),
        AbiValue::Uint(256, BigUint::from_bytes_be(v.as_array())),
    },
    SplitDepth => |v| {
        AbiValue::Uint(5, BigUint::from(v.into_bit_len())),
        AbiValue::Uint(5, BigUint::from(v.into_bit_len())),
    },
    Uint9 => |v| {
        AbiValue::Uint(9, BigUint::from(v.into_inner())),
        AbiValue::Uint(9, BigUint::from(v.into_inner())),
    },
    Uint12 => |v| {
        AbiValue::Uint(12, BigUint::from(v.into_inner())),
        AbiValue::Uint(12, BigUint::from(v.into_inner())),
    },
    Uint15 => |v| {
        AbiValue::Uint(15, BigUint::from(v.into_inner())),
        AbiValue::Uint(15, BigUint::from(v.into_inner())),
    },

    i8 => |v| {
        AbiValue::Int(8, BigInt::from(*v)),
        AbiValue::Int(8, BigInt::from(v)),
    },
    i16 => |v| {
        AbiValue::Int(16, BigInt::from(*v)),
        AbiValue::Int(16, BigInt::from(v)),
    },
    i32 => |v| {
        AbiValue::Int(32, BigInt::from(*v)),
        AbiValue::Int(32, BigInt::from(v)),
    },
    i64 => |v| {
        AbiValue::Int(64, BigInt::from(*v)),
        AbiValue::Int(64, BigInt::from(v)),
    },
    i128 => |v| {
        AbiValue::Int(128, BigInt::from(*v)),
        AbiValue::Int(128, BigInt::from(v)),
    },

    bool => |v| {
        AbiValue::Bool(*v),
        AbiValue::Bool(v),
    },

    VarUint24 => |v| {
        AbiValue::VarUint(NonZeroU8::new(4).unwrap(), BigUint::from(v.into_inner())),
        AbiValue::VarUint(NonZeroU8::new(4).unwrap(), BigUint::from(v.into_inner())),
    },
    VarUint56 => |v| {
        AbiValue::VarUint(NonZeroU8::new(8).unwrap(), BigUint::from(v.into_inner())),
        AbiValue::VarUint(NonZeroU8::new(8).unwrap(), BigUint::from(v.into_inner())),
    },

    Tokens => |v| {
        AbiValue::Token(*v),
        AbiValue::Token(v),
    },

    Cell => |v| {
        AbiValue::Cell(v.clone()),
        AbiValue::Cell(v),
    },

    Bytes => |v| {
        AbiValue::Bytes(v.clone()),
        AbiValue::Bytes(v),
    },
    String => |v| {
        AbiValue::String(v.clone()),
        AbiValue::String(v),
    },

    AnyAddr => |v| {
        AbiValue::Address(Box::new(v.clone())),
        AbiValue::Address(Box::new(v)),
    },

    ExtAddr => |v| {
        AbiValue::Address(Box::new(v.clone().into())),
        AbiValue::Address(Box::new(v.into())),
    },


    IntAddr => |v| {
        AbiValue::Address(Box::new(v.clone().into())),
        AbiValue::Address(Box::new(v.into())),
    },
    StdAddr => |v| {
        AbiValue::Address(Box::new(v.clone().into())),
        AbiValue::Address(Box::new(v.into())),
    },
    VarAddr => |v| {
        AbiValue::Address(Box::new(v.clone().into())),
        AbiValue::Address(Box::new(v.into())),
    },
}

impl IntoAbi for str {
    #[inline]
    fn as_abi(&self) -> AbiValue {
        AbiValue::String(self.to_owned())
    }

    #[inline]
    #[allow(dead_code)]
    fn into_abi(self) -> AbiValue
    where
        for<'a> str: Sized,
    {
        unreachable!()
    }
}

impl<T: WithAbiType + IntoAbi> IntoAbi for [T] {
    fn as_abi(&self) -> AbiValue {
        if typeid::of::<T>() == typeid::of::<u8>() {
            // SAFETY: T is definitely u8
            let bytes = unsafe { std::slice::from_raw_parts(self.as_ptr().cast(), self.len()) };
            AbiValue::Bytes(Bytes::copy_from_slice(bytes))
        } else {
            AbiValue::Array(
                Arc::new(T::abi_type()),
                self.iter().map(T::as_abi).collect(),
            )
        }
    }

    #[inline]
    #[allow(dead_code)]
    fn into_abi(self) -> AbiValue
    where
        for<'a> [T]: Sized,
    {
        unreachable!()
    }
}

impl<T: WithAbiType + IntoAbi, const N: usize> IntoAbi for [T; N] {
    fn as_abi(&self) -> AbiValue {
        if typeid::of::<T>() == typeid::of::<u8>() {
            // SAFETY: T is definitely u8
            let bytes = unsafe { std::slice::from_raw_parts(self.as_ptr().cast(), self.len()) };
            AbiValue::FixedBytes(Bytes::copy_from_slice(bytes))
        } else {
            AbiValue::FixedArray(
                Arc::new(T::abi_type()),
                self.iter().map(T::as_abi).collect(),
            )
        }
    }

    fn into_abi(self) -> AbiValue
    where
        Self: Sized,
    {
        if typeid::of::<T>() == typeid::of::<u8>() {
            // SAFETY: T is definitely u8
            let bytes = unsafe { std::slice::from_raw_parts(self.as_ptr().cast(), self.len()) };
            AbiValue::FixedBytes(Bytes::copy_from_slice(bytes))
        } else {
            AbiValue::FixedArray(
                Arc::new(T::abi_type()),
                self.into_iter().map(T::into_abi).collect(),
            )
        }
    }
}

impl<T: WithAbiType + IntoAbi> IntoAbi for Vec<T> {
    #[inline]
    fn as_abi(&self) -> AbiValue {
        <[T]>::as_abi(self.as_slice())
    }

    fn into_abi(self) -> AbiValue
    where
        Self: Sized,
    {
        if typeid::of::<T>() == typeid::of::<u8>() {
            // SAFETY: `T` is the same type as `u8`.
            AbiValue::Bytes(Bytes::from(unsafe { cast_vec::<T, u8>(self) }))
        } else {
            AbiValue::Array(
                Arc::new(T::abi_type()),
                self.into_iter().map(T::into_abi).collect(),
            )
        }
    }
}

impl<K: WithPlainAbiType + IntoPlainAbi, V: WithAbiType + IntoAbi> IntoAbi for BTreeMap<K, V> {
    fn as_abi(&self) -> AbiValue {
        AbiValue::Map(
            K::plain_abi_type(),
            Arc::new(V::abi_type()),
            self.iter()
                .map(|(key, value)| (K::as_plain_abi(key), V::as_abi(value)))
                .collect(),
        )
    }

    fn into_abi(self) -> AbiValue
    where
        Self: Sized,
    {
        AbiValue::Map(
            K::plain_abi_type(),
            Arc::new(V::abi_type()),
            self.into_iter()
                .map(|(key, value)| (K::into_plain_abi(key), V::into_abi(value)))
                .collect(),
        )
    }
}

impl<K: WithPlainAbiType + IntoPlainAbi, V: WithAbiType + IntoAbi, S> IntoAbi for HashMap<K, V, S> {
    fn as_abi(&self) -> AbiValue {
        AbiValue::Map(
            K::plain_abi_type(),
            Arc::new(V::abi_type()),
            self.iter()
                .map(|(key, value)| (K::as_plain_abi(key), V::as_abi(value)))
                .collect(),
        )
    }

    fn into_abi(self) -> AbiValue
    where
        Self: Sized,
    {
        AbiValue::Map(
            K::plain_abi_type(),
            Arc::new(V::abi_type()),
            self.into_iter()
                .map(|(key, value)| (K::into_plain_abi(key), V::into_abi(value)))
                .collect(),
        )
    }
}

impl<T: WithAbiType + IntoAbi> IntoAbi for Option<T> {
    #[inline]
    fn as_abi(&self) -> AbiValue {
        AbiValue::Optional(
            Arc::new(T::abi_type()),
            self.as_ref().map(T::as_abi).map(Box::new),
        )
    }

    #[inline]
    fn into_abi(self) -> AbiValue
    where
        Self: Sized,
    {
        AbiValue::Optional(Arc::new(T::abi_type()), self.map(T::into_abi).map(Box::new))
    }
}

impl IntoAbi for () {
    #[inline]
    fn as_abi(&self) -> AbiValue {
        AbiValue::Tuple(Vec::new())
    }

    #[inline]
    fn into_abi(self) -> AbiValue
    where
        Self: Sized,
    {
        self.as_abi()
    }
}

macro_rules! impl_into_abi_for_tuple {
    ($($i:tt: $t:ident),+$(,)?) => {
        impl<$($t: IntoAbi),*> IntoAbi for ($($t),+,) {
            fn as_abi(&self) -> AbiValue {
                AbiValue::Tuple(vec![
                    $(NamedAbiValue::from_index($i, <$t as IntoAbi>::as_abi(&self.$i))),*
                ])
            }

            fn into_abi(self) -> AbiValue
            where
                Self: Sized,
            {
                AbiValue::Tuple(vec![
                    $(NamedAbiValue::from_index($i, <$t as IntoAbi>::into_abi(self.$i))),*
                ])
            }
        }
    };
}

impl_into_abi_for_tuple! { 0: T0 }
impl_into_abi_for_tuple! { 0: T0, 1: T1 }
impl_into_abi_for_tuple! { 0: T0, 1: T1, 2: T2 }
impl_into_abi_for_tuple! { 0: T0, 1: T1, 2: T2, 3: T3 }
impl_into_abi_for_tuple! { 0: T0, 1: T1, 2: T2, 3: T3, 4: T4 }
impl_into_abi_for_tuple! { 0: T0, 1: T1, 2: T2, 3: T3, 4: T4, 5: T5 }
impl_into_abi_for_tuple! { 0: T0, 1: T1, 2: T2, 3: T3, 4: T4, 5: T5, 6: T6 }

/// A type which can be converted from a plain ABI value.
pub trait FromPlainAbi: Sized {
    /// Constructs self from the plain ABI value.
    fn from_plain_abi(value: PlainAbiValue) -> Result<Self>;
}

/// A type which can be converted from an ABI value.
pub trait FromAbi: Sized {
    /// Constructs self from the ABI value.
    fn from_abi(value: AbiValue) -> Result<Self>;
}

fn expected_type(expected: &'static str, value: &AbiValue) -> anyhow::Error {
    anyhow::Error::from(crate::abi::error::AbiError::TypeMismatch {
        expected: Box::from(expected),
        ty: value.display_type().to_string().into(),
    })
}

fn expected_plain_type(expected: &'static str, value: &PlainAbiValue) -> anyhow::Error {
    anyhow::Error::from(crate::abi::error::AbiError::TypeMismatch {
        expected: Box::from(expected),
        ty: value.display_type().to_string().into(),
    })
}

macro_rules! impl_from_abi_for_int {
    ($($ty:ty => ($variant:ident($bits:literal), $s:literal, $expr:tt)),*$(,)?) => {$(
        impl FromAbi for $ty {
            fn from_abi(value: AbiValue) -> Result<Self> {
                match &value {
                    AbiValue::$variant($bits, v) => match ToPrimitive::$expr(v) {
                        Some(value) => Ok(value),
                        None => Err(anyhow::Error::from(crate::error::Error::IntOverflow)),
                    },
                    value => Err(expected_type($s, value)),
                }
            }
        }

        impl FromPlainAbi for $ty {
            fn from_plain_abi(value: PlainAbiValue) -> Result<Self> {
                match &value {
                    PlainAbiValue::$variant($bits, v) => match ToPrimitive::$expr(v) {
                        Some(value) => Ok(value),
                        None => Err(anyhow::Error::from(crate::error::Error::IntOverflow)),
                    },
                    value => Err(expected_plain_type($s, value)),
                }
            }
        }
    )*};
}

impl_from_abi_for_int! {
    u8 => (Uint(8), "uint8", to_u8),
    u16 => (Uint(16), "uint16", to_u16),
    u32 => (Uint(32), "uint32", to_u32),
    u64 => (Uint(64), "uint64", to_u64),
    u128 => (Uint(128), "uint128", to_u128),

    i8 => (Int(8), "int8", to_i8),
    i16 => (Int(16), "int16", to_i16),
    i32 => (Int(32), "int32", to_i32),
    i64 => (Int(64), "int64", to_i64),
    i128 => (Int(128), "int128", to_i128),
}

impl FromAbi for AbiValue {
    #[inline]
    fn from_abi(value: AbiValue) -> Result<Self> {
        Ok(value)
    }
}

impl FromAbi for bool {
    fn from_abi(value: AbiValue) -> Result<Self> {
        match &value {
            AbiValue::Bool(value) => Ok(*value),
            value => Err(expected_type("bool", value)),
        }
    }
}

impl FromPlainAbi for bool {
    fn from_plain_abi(value: PlainAbiValue) -> Result<Self> {
        match &value {
            PlainAbiValue::Bool(value) => Ok(*value),
            value => Err(expected_plain_type("bool", value)),
        }
    }
}

impl FromAbi for VarUint24 {
    fn from_abi(value: AbiValue) -> Result<Self> {
        match &value {
            AbiValue::VarUint(size, v) if size.get() == 4 => match v.to_u32() {
                Some(value) => Ok(Self::new(value)),
                None => Err(anyhow::Error::from(crate::error::Error::IntOverflow)),
            },
            value => Err(expected_type("varuint4", value)),
        }
    }
}

impl FromAbi for VarUint56 {
    fn from_abi(value: AbiValue) -> Result<Self> {
        match &value {
            AbiValue::VarUint(size, v) if size.get() == 8 => match v.to_u64() {
                Some(value) => Ok(Self::new(value)),
                None => Err(anyhow::Error::from(crate::error::Error::IntOverflow)),
            },
            value => Err(expected_type("varuint8", value)),
        }
    }
}

impl FromAbi for Tokens {
    fn from_abi(value: AbiValue) -> Result<Self> {
        match value {
            AbiValue::VarUint(size, v) if size.get() == 16 => match v.to_u128() {
                Some(value) => Ok(Self::new(value)),
                None => Err(anyhow::Error::from(crate::error::Error::IntOverflow)),
            },
            AbiValue::Token(tokens) => Ok(tokens),
            value => Err(expected_type("varuint8", &value)),
        }
    }
}

impl FromAbi for HashBytes {
    fn from_abi(value: AbiValue) -> Result<Self> {
        match &value {
            AbiValue::Uint(256, v) => {
                let mut result = HashBytes::ZERO;

                let bytes = v.to_bytes_be();
                let bytes_len = bytes.len();
                match 32usize.checked_sub(bytes_len) {
                    None => result.0.copy_from_slice(&bytes[bytes_len - 32..]),
                    Some(pad) => result.0[pad..].copy_from_slice(&bytes),
                };

                Ok(result)
            }
            value => Err(expected_type("uint256", value)),
        }
    }
}

impl FromPlainAbi for HashBytes {
    fn from_plain_abi(value: PlainAbiValue) -> Result<Self> {
        match &value {
            PlainAbiValue::Uint(256, v) => {
                let mut result = HashBytes::ZERO;

                let bytes = v.to_bytes_be();
                let bytes_len = bytes.len();
                match 32usize.checked_sub(bytes_len) {
                    None => result.0.copy_from_slice(&bytes[bytes_len - 32..]),
                    Some(pad) => result.0[pad..].copy_from_slice(&bytes),
                };

                Ok(result)
            }
            value => Err(expected_plain_type("uint256", value)),
        }
    }
}

impl FromAbi for Cell {
    fn from_abi(value: AbiValue) -> Result<Self> {
        match value {
            AbiValue::Cell(cell) => Ok(cell),
            value => Err(expected_type("cell", &value)),
        }
    }
}

impl FromAbi for Bytes {
    fn from_abi(value: AbiValue) -> Result<Self> {
        match value {
            AbiValue::Bytes(bytes) | AbiValue::FixedBytes(bytes) => Ok(bytes),
            value => Err(expected_type("bytes or fixedbytes", &value)),
        }
    }
}

impl FromAbi for String {
    fn from_abi(value: AbiValue) -> Result<Self> {
        match value {
            AbiValue::String(string) => Ok(string),
            value => Err(expected_type("string", &value)),
        }
    }
}

impl FromAbi for AnyAddr {
    fn from_abi(value: AbiValue) -> Result<Self> {
        match value {
            AbiValue::Address(address) => Ok(*address),
            value => Err(expected_type("address", &value)),
        }
    }
}

impl FromAbi for IntAddr {
    fn from_abi(value: AbiValue) -> Result<Self> {
        if let AbiValue::Address(address) = &value {
            match address.as_ref() {
                AnyAddr::Std(addr) => return Ok(IntAddr::Std(addr.clone())),
                AnyAddr::Var(addr) => return Ok(IntAddr::Var(addr.clone())),
                AnyAddr::None | AnyAddr::Ext(_) => (),
            }
        }
        Err(expected_type("int address", &value))
    }
}

impl FromPlainAbi for IntAddr {
    fn from_plain_abi(value: PlainAbiValue) -> Result<Self> {
        match value {
            PlainAbiValue::Address(address) => Ok(*address),
            value => Err(expected_plain_type("address", &value)),
        }
    }
}

impl FromAbi for StdAddr {
    fn from_abi(value: AbiValue) -> Result<Self> {
        if let AbiValue::Address(address) = &value {
            if let AnyAddr::Std(address) = address.as_ref() {
                return Ok(address.clone());
            }
        }
        Err(expected_type("std address", &value))
    }
}

impl FromPlainAbi for StdAddr {
    fn from_plain_abi(value: PlainAbiValue) -> Result<Self> {
        if let PlainAbiValue::Address(address) = &value {
            if let IntAddr::Std(address) = address.as_ref() {
                return Ok(address.clone());
            }
        }
        Err(expected_plain_type("std address", &value))
    }
}

impl FromAbi for VarAddr {
    fn from_abi(value: AbiValue) -> Result<Self> {
        if let AbiValue::Address(address) = &value {
            if let AnyAddr::Var(address) = address.as_ref() {
                return Ok(address.clone());
            }
        }
        Err(expected_type("var address", &value))
    }
}

impl FromAbi for ExtAddr {
    fn from_abi(value: AbiValue) -> Result<Self> {
        if let AbiValue::Address(address) = &value {
            if let AnyAddr::Ext(address) = address.as_ref() {
                return Ok(address.clone());
            }
        }
        Err(expected_type("ext address", &value))
    }
}

// impl FromAbi for Option<ExtAddr> {
//     fn from_abi(value: AbiValue) -> Result<Self> {
//         if let AbiValue::Address(address) = &value {
//             match address.as_ref() {
//                 AnyAddr::Std(_) | AnyAddr::Var(_) => (),
//                 AnyAddr::Ext(addr) => return Ok(Some(addr.clone())),
//                 AnyAddr::None => return Ok(None),
//             }
//         }
//         Err(expected_type("ext address", &value))
//     }
// }

impl FromPlainAbi for VarAddr {
    fn from_plain_abi(value: PlainAbiValue) -> Result<Self> {
        if let PlainAbiValue::Address(address) = &value {
            if let IntAddr::Var(address) = address.as_ref() {
                return Ok(address.clone());
            }
        }
        Err(expected_plain_type("var address", &value))
    }
}

impl<T: FromAbi> FromAbi for Vec<T> {
    fn from_abi(value: AbiValue) -> Result<Self> {
        if typeid::of::<T>() == typeid::of::<u8>() {
            match value {
                AbiValue::Bytes(bytes) | AbiValue::FixedBytes(bytes) => {
                    let bytes = Vec::<u8>::from(bytes);
                    // SAFETY: `T` is the same type as `u8`.
                    Ok(unsafe { cast_vec::<u8, T>(bytes) })
                }
                value => Err(expected_type("bytes or fixedbytes", &value)),
            }
        } else {
            let items = match value {
                AbiValue::Array(_, items) | AbiValue::FixedArray(_, items) => items,
                value => return Err(expected_type("array", &value)),
            };
            let mut result = Vec::with_capacity(items.len());
            for item in items {
                result.push(ok!(T::from_abi(item)));
            }
            Ok(result)
        }
    }
}

impl<K: FromPlainAbi + Ord, V: FromAbi> FromAbi for BTreeMap<K, V> {
    fn from_abi(value: AbiValue) -> Result<Self> {
        match value {
            AbiValue::Map(_, _, map) => {
                let mut result = BTreeMap::new();
                for (key, value) in map {
                    result.insert(ok!(K::from_plain_abi(key)), ok!(V::from_abi(value)));
                }
                Ok(result)
            }
            value => Err(expected_type("map", &value)),
        }
    }
}

impl<K: FromPlainAbi + Eq + Hash, V: FromAbi, S: BuildHasher + Default> FromAbi
    for HashMap<K, V, S>
{
    fn from_abi(value: AbiValue) -> Result<Self> {
        match value {
            AbiValue::Map(_, _, map) => {
                let mut result = HashMap::with_capacity_and_hasher(map.len(), S::default());
                for (key, value) in map {
                    result.insert(ok!(K::from_plain_abi(key)), ok!(V::from_abi(value)));
                }
                Ok(result)
            }
            value => Err(expected_type("map", &value)),
        }
    }
}

impl<T: FromAbi> FromAbi for Option<T> {
    fn from_abi(value: AbiValue) -> Result<Self> {
        match value {
            AbiValue::Optional(_, value) => match value {
                Some(value) => T::from_abi(*value).map(Some),
                None => Ok(None),
            },
            value => Err(expected_type("optional", &value)),
        }
    }
}

impl FromAbi for () {
    fn from_abi(value: AbiValue) -> Result<Self> {
        if let AbiValue::Tuple(items) = &value {
            if items.is_empty() {
                return Ok(());
            }
        }
        Err(expected_type("()", &value))
    }
}

macro_rules! impl_from_abi_for_tuple {
    ($len:literal => $($t:ident),+$(,)?) => {
        impl<$($t: FromAbi),*> FromAbi for ($($t),+,) {
            fn from_abi(value: AbiValue) -> Result<Self> {
                match value {
                    AbiValue::Tuple(items) if items.len() == $len => {
                        let mut items = items.into_iter();
                        Ok(($(ok!(<$t as FromAbi>::from_abi(items.next().expect("exists").value))),*,))
                    }
                    value => Err(expected_type("tuple", &value))
                }
            }
        }
    };
}

impl_from_abi_for_tuple! { 1 => T0 }
impl_from_abi_for_tuple! { 2 => T0, T1 }
impl_from_abi_for_tuple! { 3 => T0, T1, T2 }
impl_from_abi_for_tuple! { 4 => T0, T1, T2, T3 }
impl_from_abi_for_tuple! { 5 => T0, T1, T2, T3, T4 }
impl_from_abi_for_tuple! { 6 => T0, T1, T2, T3, T4, T5 }
impl_from_abi_for_tuple! { 7 => T0, T1, T2, T3, T4, T5, T6 }

impl<T: FromAbi> FromAbi for Box<T> {
    #[inline]
    fn from_abi(value: AbiValue) -> Result<Self> {
        T::from_abi(value).map(Box::new)
    }
}

impl<T: FromAbi> FromAbi for Arc<T> {
    #[inline]
    fn from_abi(value: AbiValue) -> Result<Self> {
        T::from_abi(value).map(Arc::new)
    }
}

impl<T: FromAbi> FromAbi for Rc<T> {
    #[inline]
    fn from_abi(value: AbiValue) -> Result<Self> {
        T::from_abi(value).map(Rc::new)
    }
}

/// A wrapper around ABI values iterator that converts
/// each item using the [`FromAbi`] trait.
///
/// It should be used to parse fields as tuple items
/// for some struct `T` (which must implement [`WithAbiType`]).
pub trait FromAbiIter<T> {
    /// Advances the iterator and returns the next value.
    fn next_value<V: FromAbi>(&mut self) -> Result<V>;
}

impl<T, I> FromAbiIter<T> for I
where
    T: WithAbiType,
    I: Iterator<Item = NamedAbiValue>,
{
    fn next_value<V: FromAbi>(&mut self) -> Result<V> {
        match Iterator::next(self) {
            Some(item) => V::from_abi(item.value),
            None => Err(anyhow::Error::from(
                crate::abi::error::AbiError::TypeMismatch {
                    expected: T::abi_type().to_string().into_boxed_str(),
                    ty: Box::from("tuple part"),
                },
            )),
        }
    }
}

/// # Safety
///
/// The following must be true:
/// - `T1` must have the same memory layout as `T2`.
unsafe fn cast_vec<T1, T2>(v: Vec<T1>) -> Vec<T2> {
    // The code is the same as in the offical example:
    // https://doc.rust-lang.org/stable/std/vec/struct.Vec.html#method.from_raw_parts

    // Prevent running `self`'s destructor so we are in complete control
    // of the allocation.
    let mut v = std::mem::ManuallyDrop::new(v);

    // Pull out the various important pieces of information about `v`
    let p = v.as_mut_ptr().cast::<T2>();
    let len = v.len();
    let cap = v.capacity();

    unsafe { Vec::<T2>::from_raw_parts(p, len, cap) }
}

#[cfg(test)]
mod tests {
    use ahash::HashSet;

    use super::*;
    use crate::prelude::CellFamily;

    #[test]
    fn tuple_to_abi() {
        let target_abi = AbiValue::unnamed_tuple([
            AbiValue::uint(32, 123u32),
            AbiValue::uint(32, 321u32),
            AbiValue::Bool(true),
            AbiValue::unnamed_tuple([
                AbiValue::Cell(Cell::empty_cell()),
                AbiValue::Optional(Arc::new(AbiType::Bool), None),
            ]),
        ]);

        let abi = (123u32, 321u32, true, (Cell::empty_cell(), None::<bool>));

        assert_eq!(abi.into_abi(), target_abi);
    }

    #[test]
    fn entities_without_name() {
        let only_signatures = HashSet::from_iter(
            [
                u32::abi_type().named("u32"),
                bool::abi_type().named("bool"),
                <(u32, bool)>::abi_type().named("(u32,bool)"),
            ]
            .map(WithoutName),
        );

        assert!(only_signatures.contains(u32::abi_type().named("qwe").ignore_name()));
        assert!(only_signatures.contains(u32::abi_type().ignore_name()));

        assert!(only_signatures.contains(bool::abi_type().named("asd").ignore_name()));
        assert!(only_signatures.contains(bool::abi_type().ignore_name()));

        assert!(only_signatures.contains(<(u32, bool)>::abi_type().named("zxc").ignore_name()));
        assert!(only_signatures.contains(<(u32, bool)>::abi_type().ignore_name()));

        assert!(!only_signatures.contains(u64::abi_type().ignore_name()));
    }
}
