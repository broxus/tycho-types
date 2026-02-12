use std::collections::BTreeMap;
use std::num::NonZeroU8;
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use num_bigint::{BigInt, BigUint};
use num_traits::ToPrimitive;
use serde::Deserialize;
use serde::de::DeserializeSeed;
use serde::ser::{SerializeMap, SerializeSeq};

use super::ty::*;
use super::{IntoAbi, IntoPlainAbi, WithAbiType, WithPlainAbiType, WithoutName};
use crate::abi::error::AbiError;
use crate::boc::Boc;
use crate::cell::{Cell, CellFamily};
use crate::models::{AnyAddr, ExtAddr, IntAddr, StdAddr, StdAddrFormat};
use crate::num::Tokens;
use crate::util::BigIntExt;

mod de;
pub(crate) mod ser;

/// ABI value with name.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct NamedAbiValue {
    /// Item name.
    pub name: Arc<str>,
    /// ABI value.
    pub value: AbiValue,
}

impl NamedAbiValue {
    /// Ensures that all values satisfy the provided types.
    pub fn check_types(items: &[Self], types: &[NamedAbiType]) -> Result<()> {
        anyhow::ensure!(Self::have_types(items, types), AbiError::TypeMismatch {
            expected: DisplayTupleType(types).to_string().into(),
            ty: DisplayTupleValueType(items).to_string().into(),
        });
        Ok(())
    }

    /// Returns whether all values satisfy the provided types.
    pub fn have_types(items: &[Self], types: &[NamedAbiType]) -> bool {
        items.len() == types.len()
            && items
                .iter()
                .zip(types.iter())
                .all(|(item, t)| item.value.has_type(&t.ty))
    }

    /// Creates a named ABI value with an index name (e.g. `value123`).
    pub fn from_index(index: usize, value: AbiValue) -> Self {
        Self {
            name: format!("value{index}").into(),
            value,
        }
    }

    /// Ensures that value satisfies the type.
    pub fn check_type<T: AsRef<AbiType>>(&self, ty: T) -> Result<()> {
        fn type_mismatch(this: &NamedAbiValue, expected: &AbiType) -> AbiError {
            AbiError::TypeMismatch {
                expected: expected.to_string().into(),
                ty: this.value.display_type().to_string().into(),
            }
        }
        let ty = ty.as_ref();
        anyhow::ensure!(self.value.has_type(ty), type_mismatch(self, ty));
        Ok(())
    }

    /// Parses a tuple of [`NamedAbiValue`] from JSON using the provided [`NamedAbiType`]s.
    pub fn tuple_from_json_str(
        s: &str,
        types: &[NamedAbiType],
    ) -> Result<Vec<Self>, serde_path_to_error::Error<serde_json::Error>> {
        let jd = &mut serde_json::Deserializer::from_str(s);
        let mut track = serde_path_to_error::Track::new();
        match (DeserializeAbiValues { types })
            .deserialize(serde_path_to_error::Deserializer::new(&mut *jd, &mut track))
        {
            Ok(values) => {
                if let Err(e) = jd.end() {
                    return Err(serde_path_to_error::Error::new(track.path(), e));
                }

                debug_assert_eq!(values.len(), types.len());
                Ok(values)
            }
            Err(e) => Err(serde_path_to_error::Error::new(track.path(), e)),
        }
    }
}

impl From<(String, AbiValue)> for NamedAbiValue {
    #[inline]
    fn from((name, value): (String, AbiValue)) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

impl<'a> From<(&'a str, AbiValue)> for NamedAbiValue {
    #[inline]
    fn from((name, value): (&'a str, AbiValue)) -> Self {
        Self {
            name: Arc::from(name),
            value,
        }
    }
}

impl From<(usize, AbiValue)> for NamedAbiValue {
    #[inline]
    fn from((index, value): (usize, AbiValue)) -> Self {
        Self::from_index(index, value)
    }
}

impl NamedAbiType {
    /// Returns a default value corresponding to the this type.
    pub fn make_default_value(&self) -> NamedAbiValue {
        NamedAbiValue {
            name: self.name.clone(),
            value: self.ty.make_default_value(),
        }
    }
}

impl PartialEq for WithoutName<NamedAbiValue> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        WithoutName::wrap(&self.0.value).eq(WithoutName::wrap(&other.0.value))
    }
}

impl std::borrow::Borrow<WithoutName<AbiValue>> for WithoutName<NamedAbiValue> {
    fn borrow(&self) -> &WithoutName<AbiValue> {
        WithoutName::wrap(&self.0.value)
    }
}

/// ABI encoded value.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AbiValue {
    /// Unsigned integer of n bits.
    Uint(u16, BigUint),
    /// Signed integer of n bits.
    Int(u16, BigInt),
    /// Variable-length unsigned integer of maximum n bytes.
    VarUint(NonZeroU8, BigUint),
    /// Variable-length signed integer of maximum n bytes.
    VarInt(NonZeroU8, BigInt),
    /// Boolean.
    Bool(bool),
    /// Tree of cells ([`Cell`]).
    ///
    /// [`Cell`]: crate::cell::Cell
    Cell(Cell),
    /// Any address ([`AnyAddr`]).
    ///
    /// [`AnyAddr`]: crate::models::message::AnyAddr
    Address(Box<AnyAddr>),
    /// Standard internal address ([`StdAddr`])
    /// [`StdAddr`]: crate::models::message::StdAddr
    AddressStd(Option<Box<StdAddr>>),
    /// Byte array.
    Bytes(Bytes),
    /// Byte array of fixed length.
    FixedBytes(Bytes),
    /// Utf8-encoded string.
    String(String),
    /// Variable length 120-bit integer ([`Tokens`]).
    ///
    /// [`Tokens`]: crate::num::Tokens
    Token(Tokens),
    /// Product type.
    Tuple(Vec<NamedAbiValue>),
    /// Array of ABI values.
    Array(Arc<AbiType>, Vec<Self>),
    /// Fixed-length array of ABI values.
    FixedArray(Arc<AbiType>, Vec<Self>),
    /// Sorted dictionary of ABI values.
    Map(
        PlainAbiType,
        Arc<AbiType>,
        BTreeMap<PlainAbiValue, AbiValue>,
    ),
    /// Optional value.
    Optional(Arc<AbiType>, Option<Box<Self>>),
    /// Value stored in a new cell.
    Ref(Box<Self>),
}

impl AbiValue {
    /// Returns a named ABI value.
    pub fn named<T: Into<String>>(self, name: T) -> NamedAbiValue {
        NamedAbiValue {
            name: Arc::from(name.into()),
            value: self,
        }
    }

    /// Ensures that value satisfies the type.
    pub fn check_type<T: AsRef<AbiType>>(&self, ty: T) -> Result<()> {
        fn type_mismatch(value: &AbiValue, expected: &AbiType) -> AbiError {
            AbiError::TypeMismatch {
                expected: expected.to_string().into(),
                ty: value.display_type().to_string().into(),
            }
        }
        let ty = ty.as_ref();
        anyhow::ensure!(self.has_type(ty), type_mismatch(self, ty));
        Ok(())
    }

    /// Returns whether this value has the same type as the provided one.
    pub fn has_type(&self, ty: &AbiType) -> bool {
        match (self, ty) {
            (Self::Uint(n, _), AbiType::Uint(t)) => n == t,
            (Self::Int(n, _), AbiType::Int(t)) => n == t,
            (Self::VarUint(n, _), AbiType::VarUint(t)) => n == t,
            (Self::VarInt(n, _), AbiType::VarInt(t)) => n == t,
            (Self::FixedBytes(bytes), AbiType::FixedBytes(len)) => bytes.len() == *len,
            (Self::Tuple(items), AbiType::Tuple(types)) => NamedAbiValue::have_types(items, types),
            (Self::Array(ty, _), AbiType::Array(t)) => ty == t,
            (Self::FixedArray(ty, items), AbiType::FixedArray(t, len)) => {
                items.len() == *len && ty == t
            }
            (Self::Map(key_ty, value_ty, _), AbiType::Map(k, v)) => key_ty == k && value_ty == v,
            (Self::Optional(ty, _), AbiType::Optional(t)) => ty == t,
            (Self::Ref(value), AbiType::Ref(t)) => value.has_type(t),
            (Self::Bool(_), AbiType::Bool)
            | (Self::Cell(_), AbiType::Cell)
            | (Self::Address(_), AbiType::Address)
            | (Self::AddressStd(_), AbiType::AddressStd)
            | (Self::Bytes(_), AbiType::Bytes)
            | (Self::String(_), AbiType::String)
            | (Self::Token(_), AbiType::Token) => true,
            _ => false,
        }
    }

    /// Returns an ABI type of the value.
    pub fn get_type(&self) -> AbiType {
        match self {
            AbiValue::Uint(n, _) => AbiType::Uint(*n),
            AbiValue::Int(n, _) => AbiType::Int(*n),
            AbiValue::VarUint(n, _) => AbiType::VarUint(*n),
            AbiValue::VarInt(n, _) => AbiType::VarInt(*n),
            AbiValue::Bool(_) => AbiType::Bool,
            AbiValue::Cell(_) => AbiType::Cell,
            AbiValue::Address(_) => AbiType::Address,
            AbiValue::AddressStd(_) => AbiType::AddressStd,
            AbiValue::Bytes(_) => AbiType::Bytes,
            AbiValue::FixedBytes(bytes) => AbiType::FixedBytes(bytes.len()),
            AbiValue::String(_) => AbiType::String,
            AbiValue::Token(_) => AbiType::Token,
            AbiValue::Tuple(items) => AbiType::Tuple(
                items
                    .iter()
                    .map(|item| NamedAbiType::new(item.name.clone(), item.value.get_type()))
                    .collect(),
            ),
            AbiValue::Array(ty, _) => AbiType::Array(ty.clone()),
            AbiValue::FixedArray(ty, items) => AbiType::FixedArray(ty.clone(), items.len()),
            AbiValue::Map(key_ty, value_ty, _) => AbiType::Map(*key_ty, value_ty.clone()),
            AbiValue::Optional(ty, _) => AbiType::Optional(ty.clone()),
            AbiValue::Ref(value) => AbiType::Ref(Arc::new(value.get_type())),
        }
    }

    /// Returns a printable object which will display a value type signature.
    #[inline]
    pub fn display_type(&self) -> impl std::fmt::Display + '_ {
        DisplayValueType(self)
    }

    /// Simple `uintN` constructor.
    #[inline]
    pub fn uint<T>(bits: u16, value: T) -> Self
    where
        BigUint: From<T>,
    {
        Self::Uint(bits, BigUint::from(value))
    }

    /// Simple `intN` constructor.
    #[inline]
    pub fn int<T>(bits: u16, value: T) -> Self
    where
        BigInt: From<T>,
    {
        Self::Int(bits, BigInt::from(value))
    }

    /// Simple `varuintN` constructor.
    #[inline]
    pub fn varuint<T>(size: u8, value: T) -> Self
    where
        BigUint: From<T>,
    {
        Self::VarUint(NonZeroU8::new(size).unwrap(), BigUint::from(value))
    }

    /// Simple `varintN` constructor.
    #[inline]
    pub fn varint<T>(size: u8, value: T) -> Self
    where
        BigInt: From<T>,
    {
        Self::VarInt(NonZeroU8::new(size).unwrap(), BigInt::from(value))
    }

    /// Simple `address` constructor.
    #[inline]
    pub fn address<T>(value: T) -> Self
    where
        AnyAddr: From<T>,
    {
        Self::Address(Box::new(AnyAddr::from(value)))
    }

    /// Simple `address_std` constructor.
    #[inline]
    pub fn address_std<T>(value: T) -> Self
    where
        StdAddr: From<T>,
    {
        // TODO: Use `castaway` to specialize on `Box<StdAddr>`.
        Self::AddressStd(Some(Box::new(StdAddr::from(value))))
    }

    /// Simple `bytes` constructor.
    #[inline]
    pub fn bytes<T>(value: T) -> Self
    where
        Bytes: From<T>,
    {
        Self::Bytes(Bytes::from(value))
    }

    /// Simple `bytes` constructor.
    #[inline]
    pub fn fixedbytes<T>(value: T) -> Self
    where
        Bytes: From<T>,
    {
        Self::FixedBytes(Bytes::from(value))
    }

    /// Simple `tuple` constructor.
    #[inline]
    pub fn tuple<I, T>(values: I) -> Self
    where
        I: IntoIterator<Item = T>,
        NamedAbiValue: From<T>,
    {
        Self::Tuple(values.into_iter().map(NamedAbiValue::from).collect())
    }

    /// Simple `tuple` constructor.
    #[inline]
    pub fn unnamed_tuple<I>(values: I) -> Self
    where
        I: IntoIterator<Item = AbiValue>,
    {
        Self::Tuple(
            values
                .into_iter()
                .enumerate()
                .map(|(i, value)| NamedAbiValue::from_index(i, value))
                .collect(),
        )
    }

    /// Simple `array` constructor.
    #[inline]
    pub fn array<T, I>(values: I) -> Self
    where
        T: WithAbiType + IntoAbi,
        I: IntoIterator<Item = T>,
    {
        Self::Array(
            Arc::new(T::abi_type()),
            values.into_iter().map(IntoAbi::into_abi).collect(),
        )
    }

    /// Simple `fixedarray` constructor.
    #[inline]
    pub fn fixedarray<T, I>(values: I) -> Self
    where
        T: WithAbiType + IntoAbi,
        I: IntoIterator<Item = T>,
    {
        Self::FixedArray(
            Arc::new(T::abi_type()),
            values.into_iter().map(IntoAbi::into_abi).collect(),
        )
    }

    /// Simple `map` constructor.
    #[inline]
    pub fn map<K, V, I>(entries: I) -> Self
    where
        K: WithPlainAbiType + IntoPlainAbi,
        V: WithAbiType + IntoAbi,
        I: IntoIterator<Item = (K, V)>,
    {
        Self::Map(
            K::plain_abi_type(),
            Arc::new(V::abi_type()),
            entries
                .into_iter()
                .map(|(key, value)| (key.into_plain_abi(), value.into_abi()))
                .collect(),
        )
    }

    /// Simple `optional` constructor.
    #[inline]
    pub fn optional<T>(value: Option<T>) -> Self
    where
        T: WithAbiType + IntoAbi,
    {
        Self::Optional(
            Arc::new(T::abi_type()),
            value.map(T::into_abi).map(Box::new),
        )
    }

    /// Simple `optional` constructor.
    #[inline]
    pub fn reference<T>(value: T) -> Self
    where
        T: IntoAbi,
    {
        Self::Ref(Box::new(value.into_abi()))
    }

    /// Parses [`AbiValue`] from JSON using the provided [`AbiType`].
    pub fn from_json_str(
        s: &str,
        ty: &AbiType,
    ) -> Result<Self, serde_path_to_error::Error<serde_json::Error>> {
        let jd = &mut serde_json::Deserializer::from_str(s);
        let mut track = serde_path_to_error::Track::new();
        match (DeserializeAbiValue { ty })
            .deserialize(serde_path_to_error::Deserializer::new(&mut *jd, &mut track))
        {
            Ok(value) => {
                if let Err(e) = jd.end() {
                    return Err(serde_path_to_error::Error::new(track.path(), e));
                }

                Ok(value)
            }
            Err(e) => Err(serde_path_to_error::Error::new(track.path(), e)),
        }
    }
}

impl AbiType {
    /// Returns a default value corresponding to the this type.
    pub fn make_default_value(&self) -> AbiValue {
        match self {
            AbiType::Uint(bits) => AbiValue::Uint(*bits, BigUint::default()),
            AbiType::Int(bits) => AbiValue::Int(*bits, BigInt::default()),
            AbiType::VarUint(size) => AbiValue::VarUint(*size, BigUint::default()),
            AbiType::VarInt(size) => AbiValue::VarInt(*size, BigInt::default()),
            AbiType::Bool => AbiValue::Bool(false),
            AbiType::Cell => AbiValue::Cell(Cell::empty_cell()),
            AbiType::Address => AbiValue::Address(Box::default()),
            AbiType::AddressStd => AbiValue::AddressStd(None),
            AbiType::Bytes => AbiValue::Bytes(Bytes::default()),
            AbiType::FixedBytes(len) => AbiValue::FixedBytes(Bytes::from(vec![0u8; *len])),
            AbiType::String => AbiValue::String(String::default()),
            AbiType::Token => AbiValue::Token(Tokens::ZERO),
            AbiType::Tuple(items) => {
                let mut tuple = Vec::with_capacity(items.len());
                for item in items.as_ref() {
                    tuple.push(item.make_default_value());
                }
                AbiValue::Tuple(tuple)
            }
            AbiType::Array(ty) => AbiValue::Array(ty.clone(), Vec::new()),
            AbiType::FixedArray(ty, items) => {
                AbiValue::FixedArray(ty.clone(), vec![ty.make_default_value(); *items])
            }
            AbiType::Map(key_ty, value_ty) => {
                AbiValue::Map(*key_ty, value_ty.clone(), BTreeMap::default())
            }
            AbiType::Optional(ty) => AbiValue::Optional(ty.clone(), None),
            AbiType::Ref(ty) => AbiValue::Ref(Box::new(ty.make_default_value())),
        }
    }
}

impl PartialEq for WithoutName<AbiValue> {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (AbiValue::Uint(an, a), AbiValue::Uint(bn, b)) => an.eq(bn) && a.eq(b),
            (AbiValue::Int(an, a), AbiValue::Int(bn, b)) => an.eq(bn) && a.eq(b),
            (AbiValue::VarUint(an, a), AbiValue::VarUint(bn, b)) => an.eq(bn) && a.eq(b),
            (AbiValue::VarInt(an, a), AbiValue::VarInt(bn, b)) => an.eq(bn) && a.eq(b),
            (AbiValue::Bool(a), AbiValue::Bool(b)) => a.eq(b),
            (AbiValue::Cell(a), AbiValue::Cell(b)) => a.eq(b),
            (AbiValue::Address(a), AbiValue::Address(b)) => a.eq(b),
            (AbiValue::AddressStd(a), AbiValue::AddressStd(b)) => a.eq(b),
            (AbiValue::Bytes(a), AbiValue::Bytes(b)) => a.eq(b),
            (AbiValue::FixedBytes(a), AbiValue::FixedBytes(b)) => a.eq(b),
            (AbiValue::String(a), AbiValue::String(b)) => a.eq(b),
            (AbiValue::Token(a), AbiValue::Token(b)) => a.eq(b),
            (AbiValue::Tuple(a), AbiValue::Tuple(b)) => {
                WithoutName::wrap_slice(a.as_slice()).eq(WithoutName::wrap_slice(b.as_slice()))
            }
            (AbiValue::Array(at, av), AbiValue::Array(bt, bv))
            | (AbiValue::FixedArray(at, av), AbiValue::FixedArray(bt, bv)) => {
                WithoutName::wrap(at.as_ref()).eq(WithoutName::wrap(bt.as_ref()))
                    && WithoutName::wrap_slice(av.as_slice())
                        .eq(WithoutName::wrap_slice(bv.as_slice()))
            }
            (AbiValue::Map(akt, avt, a), AbiValue::Map(bkt, bvt, b)) => {
                akt.eq(bkt)
                    && WithoutName::wrap(avt.as_ref()).eq(WithoutName::wrap(bvt.as_ref()))
                    && WithoutName::wrap(a).eq(WithoutName::wrap(b))
            }
            (AbiValue::Optional(at, a), AbiValue::Optional(bt, b)) => {
                WithoutName::wrap(at.as_ref()).eq(WithoutName::wrap(bt.as_ref()))
                    && a.as_deref()
                        .map(WithoutName::wrap)
                        .eq(&b.as_deref().map(WithoutName::wrap))
            }
            (AbiValue::Ref(a), AbiValue::Ref(b)) => {
                WithoutName::wrap(a.as_ref()).eq(WithoutName::wrap(b.as_ref()))
            }
            _unused => false,
        }
    }
}

/// ABI value which has a fixed bits representation
/// and therefore can be used as a map key.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub enum PlainAbiValue {
    /// Unsigned integer of n bits.
    Uint(u16, BigUint),
    /// Signed integer of n bits.
    Int(u16, BigInt),
    /// Boolean.
    Bool(bool),
    /// Internal address ([`IntAddr`]).
    ///
    /// [`IntAddr`]: crate::models::message::IntAddr
    Address(Box<IntAddr>),
    /// Byte array of fixed length.
    FixedBytes(Bytes),
}

impl PlainAbiValue {
    /// Returns whether this value has the same type as the provided one.
    pub fn has_type(&self, ty: &PlainAbiType) -> bool {
        match (self, ty) {
            (Self::Uint(n, _), PlainAbiType::Uint(t)) => n == t,
            (Self::Int(n, _), PlainAbiType::Int(t)) => n == t,
            (Self::Bool(_), PlainAbiType::Bool) | (Self::Address(_), PlainAbiType::Address) => true,
            (Self::FixedBytes(bytes), PlainAbiType::FixedBytes(n)) => bytes.len() == *n,
            _ => false,
        }
    }

    /// Returns a printable object which will display a value type signature.
    #[inline]
    pub fn display_type(&self) -> impl std::fmt::Display + '_ {
        DisplayPlainValueType(self)
    }
}

impl From<PlainAbiValue> for AbiValue {
    fn from(value: PlainAbiValue) -> Self {
        match value {
            PlainAbiValue::Uint(n, value) => Self::Uint(n, value),
            PlainAbiValue::Int(n, value) => Self::Int(n, value),
            PlainAbiValue::Bool(value) => Self::Bool(value),
            PlainAbiValue::Address(value) => {
                let addr = match value.as_ref() {
                    IntAddr::Std(addr) => AnyAddr::Std(addr.clone()),
                    IntAddr::Var(addr) => AnyAddr::Var(addr.clone()),
                };
                AbiValue::Address(Box::new(addr))
            }
            PlainAbiValue::FixedBytes(bytes) => AbiValue::FixedBytes(bytes),
        }
    }
}

/// Contract header value.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AbiHeader {
    /// `time` header.
    Time(u64),
    /// `expire` header.
    Expire(u32),
    /// `pubkey` header.
    PublicKey(Option<Box<ed25519_dalek::VerifyingKey>>),
}

impl AbiHeader {
    /// Returns whether this value has the same type as the provided one.
    pub fn has_type(&self, ty: &AbiHeaderType) -> bool {
        matches!(
            (self, ty),
            (Self::Time(_), AbiHeaderType::Time)
                | (Self::Expire(_), AbiHeaderType::Expire)
                | (Self::PublicKey(_), AbiHeaderType::PublicKey)
        )
    }

    /// Returns a printable object which will display a header type signature.
    #[inline]
    pub fn display_type(&self) -> impl std::fmt::Display + '_ {
        DisplayHeaderType(self)
    }
}

struct DisplayHeaderType<'a>(&'a AbiHeader);

impl std::fmt::Display for DisplayHeaderType<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            AbiHeader::Time(_) => "time",
            AbiHeader::Expire(_) => "expire",
            AbiHeader::PublicKey(_) => "pubkey",
        })
    }
}

struct DisplayPlainValueType<'a>(&'a PlainAbiValue);

impl std::fmt::Display for DisplayPlainValueType<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            PlainAbiValue::Uint(n, _) => return write!(f, "uint{n}"),
            PlainAbiValue::Int(n, _) => return write!(f, "int{n}"),
            PlainAbiValue::Bool(_) => "bool",
            PlainAbiValue::Address(_) => "address",
            PlainAbiValue::FixedBytes(bytes) => return write!(f, "fixedbytes{}", bytes.len()),
        })
    }
}

struct DisplayValueType<'a>(&'a AbiValue);

impl std::fmt::Display for DisplayValueType<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self.0 {
            AbiValue::Uint(n, _) => return write!(f, "uint{n}"),
            AbiValue::Int(n, _) => return write!(f, "int{n}"),
            AbiValue::VarUint(n, _) => return write!(f, "varuint{n}"),
            AbiValue::VarInt(n, _) => return write!(f, "varint{n}"),
            AbiValue::Bool(_) => "bool",
            AbiValue::Cell(_) => "cell",
            AbiValue::Address(_) => "address",
            AbiValue::AddressStd(_) => "address_std",
            AbiValue::Bytes(_) => "bytes",
            AbiValue::FixedBytes(bytes) => return write!(f, "fixedbytes{}", bytes.len()),
            AbiValue::String(_) => "string",
            AbiValue::Token(_) => "gram",
            AbiValue::Tuple(items) => {
                return std::fmt::Display::fmt(&DisplayTupleValueType(items), f);
            }
            AbiValue::Array(ty, _) => return write!(f, "{ty}[]"),
            AbiValue::FixedArray(ty, items) => return write!(f, "{ty}[{}]", items.len()),
            AbiValue::Map(key_ty, value_ty, _) => return write!(f, "map({key_ty},{value_ty})"),
            AbiValue::Optional(ty, _) => return write!(f, "optional({ty})"),
            AbiValue::Ref(val) => return write!(f, "ref({})", val.display_type()),
        };
        f.write_str(s)
    }
}

struct DisplayTupleValueType<'a>(&'a [NamedAbiValue]);

impl std::fmt::Display for DisplayTupleValueType<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = if self.0.is_empty() {
            "()"
        } else {
            let mut first = true;
            ok!(f.write_str("("));
            for item in self.0 {
                if !std::mem::take(&mut first) {
                    ok!(f.write_str(","));
                }
                ok!(write!(f, "{}", item.value.display_type()));
            }
            ")"
        };
        f.write_str(s)
    }
}

struct DisplayTupleType<'a>(&'a [NamedAbiType]);

impl std::fmt::Display for DisplayTupleType<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = if self.0.is_empty() {
            "()"
        } else {
            let mut first = true;
            ok!(f.write_str("("));
            for item in self.0 {
                if !std::mem::take(&mut first) {
                    ok!(f.write_str(","));
                }
                ok!(write!(f, "{}", item.ty));
            }
            ")"
        };
        f.write_str(s)
    }
}

/// Serialization params for [`AbiValue`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SerializeAbiValueParams {
    /// Integer **types** with bit width greater than 128
    /// will be serialized as hex (with `0x` prefix).
    ///
    /// Default: `false`.
    ///
    /// See [`SerializeAbiValueParams::NUMBERS_AS_HEX_THRESHOLD`].
    pub big_numbers_as_hex: bool,
    /// Integer **values** with bit len not greater than 53
    /// will be serialized as is (e.g. as JSON numbers).
    ///
    /// Default: `false`.
    ///
    /// See [`SerializeAbiValueParams::NUMBERS_AS_IS_THRESHOLD`].
    pub small_numbers_as_is: bool,
}

impl SerializeAbiValueParams {
    /// Number of bits after which the number will
    /// be serialized as hex.
    pub const NUMBERS_AS_HEX_THRESHOLD: u16 = 128;

    /// Number of bits before which the number will
    /// be serialized as is (e.g. as JSON number).
    ///
    /// See [https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Number/MAX_SAFE_INTEGER].
    pub const NUMBERS_AS_IS_THRESHOLD: u16 = 53;

    const _ASSERT: () = const {
        assert!(SerializeAbiValueParams::NUMBERS_AS_IS_THRESHOLD <= 64);
    };

    fn serialize_uint<S: serde::Serializer>(
        &self,
        value: &BigUint,
        type_bits: u16,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        if self.big_numbers_as_hex && type_bits > Self::NUMBERS_AS_HEX_THRESHOLD {
            let hex_len = type_bits.div_ceil(4) as usize;
            s.collect_str(&format_args!("0x{value:0hex_len$x}"))
        } else if self.small_numbers_as_is
            && value.bits() <= Self::NUMBERS_AS_IS_THRESHOLD as u64
            && let Some(value) = value.to_u64()
        {
            s.serialize_u64(value)
        } else {
            s.collect_str(value)
        }
    }

    fn serialize_int<S: serde::Serializer>(
        &self,
        value: &BigInt,
        type_bits: u16,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        if self.big_numbers_as_hex && type_bits > Self::NUMBERS_AS_HEX_THRESHOLD {
            let hex_len = type_bits.div_ceil(4) as usize;
            let uint = value.magnitude();
            let sign = if value.sign() == num_bigint::Sign::Minus {
                "-"
            } else {
                ""
            };
            s.collect_str(&format_args!("{sign}0x{uint:0hex_len$x}"))
        } else if self.small_numbers_as_is
            && value.bits() <= Self::NUMBERS_AS_IS_THRESHOLD as u64
            && let Some(value) = value.to_i64()
        {
            s.serialize_i64(value)
        } else {
            s.collect_str(value)
        }
    }

    fn serialize_tokens<S: serde::Serializer>(
        &self,
        value: &Tokens,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        if self.small_numbers_as_is {
            let inner = 128 - value.into_inner().leading_zeros();
            if inner <= Self::NUMBERS_AS_IS_THRESHOLD as u32 {
                return s.serialize_u64(value.into_inner() as u64);
            }
        }

        s.collect_str(value)
    }
}

/// A [`serde::Serialize`] for a tuple of [`NamedAbiValue`].
pub struct SerializeAbiValues<'a> {
    /// A tuple of values.
    pub values: &'a [NamedAbiValue],
    /// Serialization params.
    pub params: SerializeAbiValueParams,
}

impl<'a> SerializeAbiValues<'a> {
    /// Creates a serializer.
    #[inline]
    pub fn with_params(values: &'a [NamedAbiValue], params: SerializeAbiValueParams) -> Self {
        Self { values, params }
    }
}

impl serde::Serialize for SerializeAbiValues<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.values.len()))?;
        for item in self.values {
            map.serialize_entry(item.name.as_ref(), &SerializeAbiValue {
                value: &item.value,
                params: self.params,
            })?;
        }
        map.end()
    }
}

/// A [`serde::Serialize`] for [`AbiValue`].
pub struct SerializeAbiValue<'a> {
    /// Value to serialize.
    pub value: &'a AbiValue,
    /// Serialization params.
    pub params: SerializeAbiValueParams,
}

impl<'a> SerializeAbiValue<'a> {
    /// Creates a serializer.
    #[inline]
    pub fn with_params(value: &'a AbiValue, params: SerializeAbiValueParams) -> Self {
        Self { value, params }
    }
}

impl serde::Serialize for SerializeAbiValue<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use base64::prelude::{BASE64_STANDARD, Engine as _};

        struct SerdeBytes<'a> {
            bytes: &'a [u8],
            fixed: bool,
        }

        impl serde::Serialize for SerdeBytes<'_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                if s.is_human_readable() {
                    let string = if self.fixed {
                        hex::encode(self.bytes)
                    } else {
                        BASE64_STANDARD.encode(self.bytes)
                    };
                    s.serialize_str(&string)
                } else {
                    s.serialize_bytes(self.bytes)
                }
            }
        }

        struct SerdeMapKey<'a> {
            key: &'a PlainAbiValue,
            big_numbers_as_hex: bool,
        }

        impl serde::Serialize for SerdeMapKey<'_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let params = SerializeAbiValueParams {
                    big_numbers_as_hex: self.big_numbers_as_hex,
                    ..Default::default()
                };

                match self.key {
                    PlainAbiValue::Uint(bits, value) => params.serialize_uint(value, *bits, s),
                    PlainAbiValue::Int(bits, value) => params.serialize_int(value, *bits, s),
                    PlainAbiValue::Bool(value) => s.collect_str(value),
                    PlainAbiValue::Address(value) => s.collect_str(value),
                    PlainAbiValue::FixedBytes(bytes) => s.serialize_str(&hex::encode(bytes)),
                }
            }
        }

        match self.value {
            AbiValue::Uint(bits, value) => self.params.serialize_uint(value, *bits, s),
            AbiValue::Int(bits, value) => self.params.serialize_int(value, *bits, s),
            AbiValue::VarUint(bytes, value) => {
                self.params.serialize_uint(value, bytes.get() as u16 * 8, s)
            }
            AbiValue::VarInt(bytes, value) => {
                self.params.serialize_int(value, bytes.get() as u16 * 8, s)
            }
            AbiValue::Bool(value) => s.serialize_bool(*value),
            AbiValue::Cell(cell) => Boc::serialize(cell, s),
            AbiValue::Address(addr) => match addr.as_ref() {
                AnyAddr::None => s.serialize_str(""),
                AnyAddr::Std(addr) => s.collect_str(addr),
                AnyAddr::Ext(addr) => s.collect_str(addr),
                AnyAddr::Var(_) => s.serialize_str("varaddr"), // TODO: add proper support
            },
            AbiValue::AddressStd(addr) => match addr {
                None => s.serialize_str(""),
                Some(addr) => s.collect_str(addr),
            },
            AbiValue::Bytes(bytes) => SerdeBytes {
                bytes,
                fixed: false,
            }
            .serialize(s),
            AbiValue::FixedBytes(bytes) => SerdeBytes { bytes, fixed: true }.serialize(s),
            AbiValue::String(value) => s.serialize_str(value),
            AbiValue::Token(tokens) => self.params.serialize_tokens(tokens, s),
            AbiValue::Tuple(values) => SerializeAbiValues {
                values,
                params: self.params,
            }
            .serialize(s),
            AbiValue::Array(_, values) | AbiValue::FixedArray(_, values) => {
                let mut seq = s.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(&SerializeAbiValue {
                        value,
                        params: self.params,
                    })?;
                }
                seq.end()
            }
            AbiValue::Map(_, _, items) => {
                let mut map = s.serialize_map(Some(items.len()))?;
                for (key, value) in items {
                    map.serialize_entry(
                        &SerdeMapKey {
                            key,
                            big_numbers_as_hex: self.params.big_numbers_as_hex,
                        },
                        &SerializeAbiValue {
                            value,
                            params: self.params,
                        },
                    )?;
                }
                map.end()
            }
            AbiValue::Optional(_, value) => value
                .as_ref()
                .map(|value| SerializeAbiValue {
                    value,
                    params: self.params,
                })
                .serialize(s),
            AbiValue::Ref(value) => SerializeAbiValue {
                value,
                params: self.params,
            }
            .serialize(s),
        }
    }
}

/// A [`DeserializeSeed`] for a tuple of [`NamedAbiValue`].
pub struct DeserializeAbiValues<'a> {
    /// Types of tuple items.
    pub types: &'a [NamedAbiType],
}

impl<'de> serde::de::DeserializeSeed<'de> for DeserializeAbiValues<'_> {
    type Value = Vec<NamedAbiValue>;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        use serde::de::{Error, Visitor};

        struct TupleVisitor<'a>(&'a [NamedAbiType]);

        impl<'de> Visitor<'de> for TupleVisitor<'_> {
            type Value = Vec<NamedAbiValue>;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "a tuple with named fields")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut items =
                    ahash::HashMap::with_capacity_and_hasher(self.0.len(), Default::default());
                for ty in self.0 {
                    items.insert(ty.name.as_ref(), (&ty.ty, None));
                }

                while let Some(key) = map.next_key::<String>()? {
                    let Some((ty, value)) = items.get_mut(key.as_str()) else {
                        return Err(Error::custom(format_args!(
                            "unknown field `{key}` in tuple"
                        )));
                    };

                    *value = Some(map.next_value_seed(DeserializeAbiValue { ty })?);
                }

                let mut result = Vec::with_capacity(self.0.len());
                for item in self.0 {
                    if let Some((_, value)) = items.get_mut(item.name.as_ref())
                        && let Some(value) = value.take()
                    {
                        result.push(NamedAbiValue {
                            name: item.name.clone(),
                            value,
                        });
                    } else {
                        return Err(Error::custom(format_args!(
                            "missing field `{}` in tuple",
                            item.name.as_ref()
                        )));
                    }
                }

                Ok(result)
            }
        }

        d.deserialize_map(TupleVisitor(self.types))
    }
}

/// A [`DeserializeSeed`] for [`AbiValue`].
///
/// Example:
/// ```
/// # use serde::de::DeserializeSeed;
/// # use tycho_types::abi::{AbiType, AbiValue, DeserializeAbiValue};
/// let abi_type = AbiType::int(32);
/// let json = "123";
/// let value: AbiValue = DeserializeAbiValue { ty: &abi_type }
///     .deserialize(&mut serde_json::Deserializer::from_str(json))
///     .unwrap();
/// assert_eq!(value, AbiValue::int(32, 123));
/// ```
#[repr(transparent)]
pub struct DeserializeAbiValue<'a> {
    /// Value type description.
    pub ty: &'a AbiType,
}

impl<'de> serde::de::DeserializeSeed<'de> for DeserializeAbiValue<'_> {
    type Value = AbiValue;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        use std::str::FromStr;

        use base64::prelude::{BASE64_STANDARD, Engine as _};
        use num_traits::Num;
        use serde::de::{Error, Visitor};

        fn ensure_fits<V: BigIntExt, T: std::fmt::Display, E: Error>(
            v: &V,
            ty: T,
            bits: u16,
            signed: bool,
        ) -> Result<(), E> {
            let value_bits = v.bitsize(signed);
            if value_bits <= bits {
                Ok(())
            } else {
                Err(Error::custom(format_args!(
                    "parsed integer doesn't fit into `{ty}`"
                )))
            }
        }

        fn parse_common<T: std::fmt::Display, E: Error>(v: &str, ty: T) -> Result<BigUint, E> {
            let (v, radix) = if let Some(v) = v.strip_prefix("0x") {
                (v, 16)
            } else if let Some(v) = v.strip_prefix("0b") {
                (v, 2)
            } else {
                (v, 10)
            };

            BigUint::from_str_radix(v, radix)
                .map_err(|e| E::custom(format_args!("invalid {ty}: {e}")))
        }

        fn parse_uint<T: std::fmt::Display, E: Error>(
            v: &str,
            ty: T,
            bits: u16,
        ) -> Result<BigUint, E> {
            if v.starts_with('-') {
                return Err(Error::custom(format_args!("expected an unsigned integer")));
            }
            let value = parse_common(v, &ty)?;
            ensure_fits(&value, ty, bits, false)?;
            Ok(value)
        }

        fn parse_int<T: std::fmt::Display, E: Error>(
            v: &str,
            ty: T,
            bits: u16,
        ) -> Result<BigInt, E> {
            let (v, sign) = if let Some(v) = v.strip_prefix('-') {
                (v, num_bigint::Sign::Minus)
            } else {
                (v, num_bigint::Sign::Plus)
            };

            let value = BigInt::from_biguint(sign, parse_common(v, &ty)?);
            ensure_fits(&value, ty, bits, true)?;
            Ok(value)
        }

        struct PlainAbiValueSeed(PlainAbiType);

        impl<'de> serde::de::DeserializeSeed<'de> for PlainAbiValueSeed {
            type Value = PlainAbiValue;

            fn deserialize<D>(self, d: D) -> Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                d.deserialize_str(PlainAbiValueVisitor(self.0))
            }
        }

        struct PlainAbiValueVisitor(PlainAbiType);

        impl<'de> Visitor<'de> for PlainAbiValueVisitor {
            type Value = PlainAbiValue;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                match self.0 {
                    PlainAbiType::Uint(bits) => {
                        write!(f, "an {bits}-bit unsigned integer as string")
                    }
                    PlainAbiType::Int(bits) => {
                        write!(f, "a {bits}-bit signed integer as string")
                    }
                    PlainAbiType::Address => write!(f, "an address"),
                    PlainAbiType::Bool => write!(f, "a bool as string"),
                    PlainAbiType::FixedBytes(n) => write!(f, "hex-encoded {n} bytes as string"),
                }
            }

            fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
                match self.0 {
                    PlainAbiType::Uint(bits) => parse_uint(v, format_args!("uint{bits}"), bits)
                        .map(|v| PlainAbiValue::Uint(bits, v)),
                    PlainAbiType::Int(bits) => parse_int(v, format_args!("int{bits}"), bits)
                        .map(|v| PlainAbiValue::Int(bits, v)),
                    PlainAbiType::Bool => {
                        let value = bool::from_str(v)
                            .map_err(|e| Error::custom(format_args!("invalid bool: {e}")))?;
                        Ok(PlainAbiValue::Bool(value))
                    }
                    PlainAbiType::Address => {
                        let (addr, _) = StdAddr::from_str_ext(v, StdAddrFormat::any())
                            .map_err(|e| Error::custom(format_args!("invalid address: {e}")))?;
                        Ok(PlainAbiValue::Address(Box::new(IntAddr::Std(addr))))
                    }
                    PlainAbiType::FixedBytes(n) => {
                        let Some(str_len) = n.checked_mul(2) else {
                            return Err(Error::custom(format_args!(
                                "{n} bytes cannot be parsed as a hex string"
                            )));
                        };
                        if v.len() != str_len {
                            return Err(Error::custom(format_args!(
                                "invalid string length {}, expected {str_len}",
                                v.len(),
                            )));
                        }

                        let value = hex::decode(v).map_err(Error::custom)?;
                        debug_assert_eq!(value.len(), n);
                        Ok(PlainAbiValue::FixedBytes(value.into()))
                    }
                }
            }
        }

        struct UintVisitor<T>(T, u16);

        impl<'de, T: std::fmt::Display> Visitor<'de> for UintVisitor<T> {
            type Value = BigUint;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "an unsigned integer as string or number")
            }

            fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
                parse_uint(v, self.0, self.1)
            }

            fn visit_u64<E: Error>(self, v: u64) -> Result<Self::Value, E> {
                let value = BigUint::from(v);
                ensure_fits(&value, self.0, self.1, false)?;
                Ok(value)
            }

            fn visit_i64<E: Error>(self, v: i64) -> Result<Self::Value, E> {
                if v >= 0 {
                    self.visit_u64(v as u64)
                } else {
                    Err(Error::custom("expected an unsigned integer"))
                }
            }
        }

        struct IntVisitor<T>(T, u16);

        impl<'de, T: std::fmt::Display> Visitor<'de> for IntVisitor<T> {
            type Value = BigInt;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "a signed integer as string or number")
            }

            fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
                parse_int(v, self.0, self.1)
            }

            fn visit_u64<E: Error>(self, v: u64) -> Result<Self::Value, E> {
                let value = BigInt::from(v);
                ensure_fits(&value, self.0, self.1, true)?;
                Ok(value)
            }

            fn visit_i64<E: Error>(self, v: i64) -> Result<Self::Value, E> {
                let value = BigInt::from(v);
                ensure_fits(&value, self.0, self.1, true)?;
                Ok(value)
            }
        }

        struct AddressVisitor {
            std_only: bool,
        }

        impl<'de> Visitor<'de> for AddressVisitor {
            type Value = AbiValue;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                if self.std_only {
                    write!(f, "an std address as string")
                } else {
                    write!(f, "an address as string")
                }
            }

            fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
                Ok(if self.std_only {
                    AbiValue::AddressStd(None)
                } else {
                    AbiValue::Address(Box::new(AnyAddr::None))
                })
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                deserializer.deserialize_str(self)
            }

            fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
                if v.is_empty() {
                    return self.visit_none();
                }

                let is_extaddr = v.starts_with(':');
                if self.std_only && (is_extaddr || v == "varaddr") {
                    return Err(Error::custom("expected an std address"));
                }

                if is_extaddr {
                    let addr = ExtAddr::from_str(v)
                        .map_err(|e| Error::custom(format_args!("invalid address: {e}")))?;
                    return Ok(AbiValue::Address(Box::new(AnyAddr::Ext(addr))));
                }

                let (addr, _) = StdAddr::from_str_ext(v, StdAddrFormat::any())
                    .map_err(|e| Error::custom(format_args!("invalid address: {e}")))?;
                Ok(if self.std_only {
                    AbiValue::AddressStd(Some(Box::new(addr)))
                } else {
                    AbiValue::Address(Box::new(AnyAddr::Std(addr)))
                })
            }
        }

        struct BytesVisitor(Option<usize>);

        impl BytesVisitor {
            fn make_value<E: Error>(&self, bytes: Vec<u8>) -> Result<AbiValue, E> {
                match self.0 {
                    None => Ok(AbiValue::Bytes(Bytes::from(bytes))),
                    Some(len) if bytes.len() == len => Ok(AbiValue::FixedBytes(Bytes::from(bytes))),
                    Some(_) => Err(Error::invalid_length(bytes.len(), self)),
                }
            }
        }

        impl<'de> Visitor<'de> for BytesVisitor {
            type Value = AbiValue;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                match self.0 {
                    None => write!(f, "base64-encoded bytes as string"),
                    Some(n) => write!(f, "hex-encoded {n} bytes as string"),
                }
            }

            fn visit_bytes<E: Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                self.make_value(v.to_vec())
            }

            fn visit_byte_buf<E: Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
                self.make_value(v)
            }

            fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
                let value = match self.0 {
                    // `bytes` are parsed as a base64 string
                    None => BASE64_STANDARD.decode(v).map_err(|e| {
                        E::custom(format_args!("failed to deserialize a base64 string: {e}"))
                    })?,
                    // `fixedbytes` are parsed as a hex string
                    Some(_) => hex::decode(v).map_err(|e| {
                        E::custom(format_args!("failed to deserialize a hex string: {e}"))
                    })?,
                };

                self.make_value(value)
            }
        }

        struct ArrayVisitor<'a>(&'a Arc<AbiType>, Option<usize>);

        impl<'de> Visitor<'de> for ArrayVisitor<'_> {
            type Value = AbiValue;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                match self.1 {
                    None => write!(f, "an array of values"),
                    Some(n) => write!(f, "an array of {n} values"),
                }
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut result = Vec::with_capacity(seq.size_hint().unwrap_or_default());
                while let Some(value) = seq.next_element_seed(DeserializeAbiValue { ty: self.0 })? {
                    result.push(value);
                }

                match self.1 {
                    None => Ok(AbiValue::Array(self.0.clone(), result)),
                    Some(len) if result.len() == len => {
                        Ok(AbiValue::FixedArray(self.0.clone(), result))
                    }
                    Some(_) => Err(Error::invalid_length(result.len(), &self)),
                }
            }
        }

        struct MapVisitor<'a>(PlainAbiType, &'a Arc<AbiType>);

        impl<'de> Visitor<'de> for MapVisitor<'_> {
            type Value = AbiValue;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "a map")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut result = BTreeMap::new();
                while let Some(key) = map.next_key_seed(PlainAbiValueSeed(self.0))? {
                    let value = map.next_value_seed(DeserializeAbiValue { ty: self.1 })?;
                    result.insert(key, value);
                }
                Ok(AbiValue::Map(self.0, self.1.clone(), result))
            }
        }

        struct OptionVisitor<'a>(&'a Arc<AbiType>);

        impl<'de> Visitor<'de> for OptionVisitor<'_> {
            type Value = AbiValue;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "an optional value")
            }

            fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
                Ok(AbiValue::Optional(self.0.clone(), None))
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = DeserializeAbiValue {
                    ty: self.0.as_ref(),
                }
                .deserialize(deserializer)?;
                Ok(AbiValue::Optional(self.0.clone(), Some(Box::new(value))))
            }
        }

        match self.ty {
            AbiType::Uint(bits) => {
                let value = d.deserialize_any(UintVisitor(format_args!("uint{bits}"), *bits))?;
                Ok(AbiValue::Uint(*bits, value))
            }
            AbiType::Int(bits) => {
                let value = d.deserialize_any(IntVisitor(format_args!("int{bits}"), *bits))?;
                Ok(AbiValue::Int(*bits, value))
            }
            AbiType::VarUint(bytes) => {
                let bytes = *bytes;
                let value = d.deserialize_any(UintVisitor(
                    format_args!("varuint{bytes}"),
                    bytes.get() as u16 * 8,
                ))?;
                Ok(AbiValue::VarUint(bytes, value))
            }
            AbiType::VarInt(bytes) => {
                let bytes = *bytes;
                let value = d.deserialize_any(IntVisitor(
                    format_args!("varint{bytes}"),
                    bytes.get() as u16 * 8,
                ))?;
                Ok(AbiValue::VarInt(bytes, value))
            }
            AbiType::Bool => bool::deserialize(d).map(AbiValue::Bool),
            AbiType::Cell => Boc::deserialize(d).map(AbiValue::Cell),
            AbiType::Address => d.deserialize_option(AddressVisitor { std_only: false }),
            AbiType::AddressStd => d.deserialize_option(AddressVisitor { std_only: true }),
            AbiType::Bytes => {
                if d.is_human_readable() {
                    d.deserialize_str(BytesVisitor(None))
                } else {
                    d.deserialize_byte_buf(BytesVisitor(None))
                }
            }
            AbiType::FixedBytes(len) => {
                if d.is_human_readable() {
                    d.deserialize_str(BytesVisitor(Some(*len)))
                } else {
                    d.deserialize_byte_buf(BytesVisitor(Some(*len)))
                }
            }
            AbiType::String => String::deserialize(d).map(AbiValue::String),
            AbiType::Token => {
                let Some(value) = d
                    .deserialize_any(UintVisitor("tokens", Tokens::VALUE_BITS))?
                    .to_u128()
                else {
                    return Err(Error::custom("tokens integer out of range"));
                };
                Ok(AbiValue::Token(Tokens::new(value)))
            }
            AbiType::Tuple(types) => DeserializeAbiValues { types }
                .deserialize(d)
                .map(AbiValue::Tuple),
            AbiType::Array(ty) => d.deserialize_seq(ArrayVisitor(ty, None)),
            AbiType::FixedArray(ty, len) => d.deserialize_seq(ArrayVisitor(ty, Some(*len))),
            AbiType::Map(key, value) => d.deserialize_map(MapVisitor(*key, value)),
            AbiType::Optional(ty) => d.deserialize_option(OptionVisitor(ty)),
            AbiType::Ref(ty) => DeserializeAbiValue { ty }
                .deserialize(d)
                .map(|value| AbiValue::Ref(Box::new(value))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::AbiVersion;
    use crate::cell::HashBytes;

    #[test]
    fn to_from_json_works() -> Result<()> {
        for (value, json) in [
            // uint
            (AbiValue::uint(32, 0u32), "0"),
            (AbiValue::uint(32, 123u32), "123"),
            (AbiValue::uint(32, 0u32), r#""0""#),
            (AbiValue::uint(32, 123u32), r#""123""#),
            (AbiValue::uint(32, 123u32), r#""0x7b""#),
            (AbiValue::uint(32, 123u32), r#""0x000007b""#),
            (AbiValue::uint(32, 123u32), r#""0b01111011""#),
            // int (positive)
            (AbiValue::int(32, 0), "0"),
            (AbiValue::int(32, 123), "123"),
            (AbiValue::int(32, 0), r#""0""#),
            (AbiValue::int(32, 123), r#""123""#),
            (AbiValue::int(32, 123), r#""0x7b""#),
            (AbiValue::int(32, 123), r#""0x000007b""#),
            (AbiValue::int(32, 123), r#""0b01111011""#),
            // int (negative)
            (AbiValue::int(32, -123), "-123"),
            (AbiValue::int(32, 0), r#""-0""#),
            (AbiValue::int(32, -123), r#""-123""#),
            (AbiValue::int(32, -123), r#""-0x7b""#),
            (AbiValue::int(32, -123), r#""-0x0000007b""#),
            (AbiValue::int(32, -123), r#""-0b01111011""#),
            // varuint
            (AbiValue::varuint(16, 0u32), "0"),
            (AbiValue::varuint(16, 123u32), "123"),
            (AbiValue::varuint(16, 0u32), r#""0""#),
            (AbiValue::varuint(16, 123u32), r#""123""#),
            (AbiValue::varuint(16, 123u32), r#""0x7b""#),
            (AbiValue::varuint(16, 123u32), r#""0b01111011""#),
            // int (positive)
            (AbiValue::varint(16, 0), "0"),
            (AbiValue::varint(16, 123), "123"),
            (AbiValue::varint(16, 0), r#""0""#),
            (AbiValue::varint(16, 123), r#""123""#),
            (AbiValue::varint(16, 123), r#""0x7b""#),
            (AbiValue::varint(16, 123), r#""0b01111011""#),
            // int (negative)
            (AbiValue::varint(16, -123), "-123"),
            (AbiValue::varint(16, 0), r#""-0""#),
            (AbiValue::varint(16, -123), r#""-123""#),
            (AbiValue::varint(16, -123), r#""-0x7b""#),
            (AbiValue::varint(16, -123), r#""-0b01111011""#),
            // bool
            (AbiValue::Bool(false), "false"),
            (AbiValue::Bool(true), "true"),
            // cell
            (
                AbiValue::Cell(Cell::empty_cell()),
                "\"te6ccgEBAQEAAgAAAA==\"",
            ),
            // address
            (AbiValue::Address(Box::new(AnyAddr::None)), "null"),
            (AbiValue::Address(Box::new(AnyAddr::None)), "\"\""),
            (
                AbiValue::Address(Box::new(AnyAddr::Std(StdAddr::new(0, HashBytes([0; 32]))))),
                "\"0:0000000000000000000000000000000000000000000000000000000000000000\"",
            ),
            (
                AbiValue::Address(Box::new(AnyAddr::Std(StdAddr::new(
                    -1,
                    HashBytes([0x33; 32]),
                )))),
                "\"Uf8zMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMxYA\"",
            ),
            (
                AbiValue::Address(Box::new(AnyAddr::Ext(ExtAddr::new(0, []).unwrap()))),
                "\":\"",
            ),
            (
                AbiValue::Address(Box::new(AnyAddr::Ext(ExtAddr::new(5, vec![0x84]).unwrap()))),
                "\":84_\"",
            ),
            // std address
            (AbiValue::AddressStd(None), "null"),
            (AbiValue::AddressStd(None), "\"\""),
            (
                AbiValue::AddressStd(Some(Box::new(StdAddr::new(0, HashBytes([0; 32]))))),
                "\"0:0000000000000000000000000000000000000000000000000000000000000000\"",
            ),
            (
                AbiValue::AddressStd(Some(Box::new(StdAddr::new(-1, HashBytes([0x33; 32]))))),
                "\"Uf8zMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMxYA\"",
            ),
            // bytes
            (AbiValue::Bytes(Bytes::new()), "\"\""),
            (AbiValue::Bytes(Bytes::from(&[0x7bu8] as &[_])), "\"ew==\""),
            // fixed bytes
            (AbiValue::FixedBytes(Bytes::new()), "\"\""),
            (
                AbiValue::FixedBytes(Bytes::from(&[0x7bu8] as &[_])),
                "\"7b\"",
            ),
            // string
            (AbiValue::String(String::new()), "\"\""),
            (AbiValue::String("hello".to_owned()), "\"hello\""),
            // token
            (AbiValue::Token(Tokens::new(0)), "0"),
            (AbiValue::Token(Tokens::new(123)), "123"),
            (AbiValue::Token(Tokens::new(0)), r#""0""#),
            (AbiValue::Token(Tokens::new(123)), r#""123""#),
            (AbiValue::Token(Tokens::new(123)), r#""0x7b""#),
            (AbiValue::Token(Tokens::new(123)), r#""0b01111011""#),
            // tuple
            (AbiValue::tuple([] as [NamedAbiValue; 0]), "{}"),
            (
                AbiValue::tuple([
                    AbiValue::Bool(false).named("hi"),
                    AbiValue::String("hello".to_owned()).named("mark"),
                ]),
                "{\"mark\":\"hello\",\"hi\":false}",
            ),
            // array
            (AbiValue::array::<bool, _>([]), "[]"),
            (AbiValue::array([0u32, 123u32]), "[0,\"123\"]"),
            // fixed array
            (AbiValue::fixedarray::<bool, _>([]), "[]"),
            (AbiValue::fixedarray([0u32, 123u32]), "[0,\"123\"]"),
            // bool map
            (AbiValue::map(std::iter::empty::<(bool, u32)>()), "{}"),
            (
                AbiValue::map([(false, 123), (true, 234)]),
                "{\"true\":234,\"false\":123}",
            ),
            // uint map
            (AbiValue::map(std::iter::empty::<(u32, u32)>()), "{}"),
            (
                AbiValue::map([(10000, 123), (1000, 234)]),
                "{\"1000\":234,\"10000\":123}",
            ),
            // uint map
            (AbiValue::map(std::iter::empty::<(u32, u32)>()), "{}"),
            (
                AbiValue::map([(HashBytes([0; 32]), 123), (HashBytes([0x33; 32]), 234)]),
                "{\"0x0000000000000000000000000000000000000000000000000000000000000000\":123,\
                \"0x3333333333333333333333333333333333333333333333333333333333333333\":234}",
            ),
            // int map
            (AbiValue::map(std::iter::empty::<(u32, u32)>()), "{}"),
            (
                AbiValue::map([(-1000, 123), (1000, 234)]),
                "{\"1000\":234,\"-1000\":123}",
            ),
            // address map
            (AbiValue::map(std::iter::empty::<(StdAddr, u32)>()), "{}"),
            (
                AbiValue::map([
                    (StdAddr::new(0, HashBytes::ZERO), 123),
                    (StdAddr::new(-1, HashBytes([0x33; 32])), 234),
                ]),
                "{\"0:0000000000000000000000000000000000000000000000000000000000000000\":123,\
                \"Uf8zMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMxYA\":234}",
            ),
            // fixedbytes map
            (
                AbiValue::map([([0x33u8; 32], 123), ([0x55; 32], 234)]),
                "{\"3333333333333333333333333333333333333333333333333333333333333333\":123,\
                \"5555555555555555555555555555555555555555555555555555555555555555\": 234}",
            ),
            // optional
            (AbiValue::optional(None::<u32>), "null"),
            (AbiValue::optional(Some(123u32)), "\"123\""),
            // ref
            (AbiValue::reference(AbiValue::uint(32, 123u32)), "\"123\""),
        ] {
            let ty = value.get_type();
            println!("parsing provided `{json}` with {ty:?}");
            assert_eq!(AbiValue::from_json_str(json, &ty)?, value,);

            let serialized =
                serde_json::to_string(&SerializeAbiValue::with_params(&value, Default::default()))?;
            println!("parsing auto `{serialized}` with {ty:?}");
            let parsed = AbiValue::from_json_str(&serialized, &ty)?;
            assert_eq!(parsed, value);
        }

        Ok(())
    }

    #[test]
    fn custom_json_params_works() -> Result<()> {
        fn check_parsed(value: &AbiValue, json: &str) {
            let parsed = AbiValue::from_json_str(json, &value.get_type()).unwrap();
            assert_eq!(value, &parsed);
        }

        //

        let params = SerializeAbiValueParams {
            big_numbers_as_hex: true,
            ..Default::default()
        };

        let value = AbiValue::map([(HashBytes([0; 32]), ()), (HashBytes([0x33; 32]), ())]);
        let json = serde_json::to_string(&SerializeAbiValue::with_params(&value, params))?;
        assert_eq!(
            json,
            "{\"0x0000000000000000000000000000000000000000000000000000000000000000\":{},\
            \"0x3333333333333333333333333333333333333333333333333333333333333333\":{}}"
        );
        check_parsed(&value, &json);

        //

        let params = SerializeAbiValueParams {
            small_numbers_as_is: true,
            ..Default::default()
        };

        let max_uint = (1u64 << SerializeAbiValueParams::NUMBERS_AS_IS_THRESHOLD) - 1;

        for (value, json) in [
            // uint
            (AbiValue::uint(256, 123u32), "123".to_owned()),
            (AbiValue::uint(256, max_uint), format!("{max_uint}")),
            (
                AbiValue::uint(256, max_uint + 1),
                format!("\"{}\"", max_uint + 1),
            ),
            (AbiValue::uint(256, u64::MAX), format!("\"{}\"", u64::MAX)),
            // int
            (AbiValue::int(64, 123), "123".to_owned()),
            (AbiValue::int(256, max_uint), format!("{max_uint}")),
            (
                AbiValue::int(256, -(max_uint as i64)),
                format!("-{max_uint}"),
            ),
            (
                AbiValue::int(256, -((max_uint + 1) as i64)),
                format!("\"-{}\"", max_uint + 1),
            ),
            (AbiValue::int(256, i64::MAX), format!("\"{}\"", i64::MAX)),
            (AbiValue::int(256, i64::MIN), format!("\"{}\"", i64::MIN)),
            // varuint
            (AbiValue::varuint(16, 123u32), "123".to_owned()),
            (AbiValue::varuint(16, max_uint), format!("{max_uint}")),
            (
                AbiValue::varuint(16, max_uint + 1),
                format!("\"{}\"", max_uint + 1),
            ),
            (AbiValue::varuint(16, u64::MAX), format!("\"{}\"", u64::MAX)),
            // varint
            (AbiValue::varint(16, 123), "123".to_owned()),
            (AbiValue::varint(16, max_uint), format!("{max_uint}")),
            (
                AbiValue::varint(16, -(max_uint as i64)),
                format!("-{max_uint}"),
            ),
            (
                AbiValue::varint(16, -((max_uint + 1) as i64)),
                format!("\"-{}\"", max_uint + 1),
            ),
            (AbiValue::varint(16, i64::MAX), format!("\"{}\"", i64::MAX)),
            (AbiValue::varint(16, i64::MIN), format!("\"{}\"", i64::MIN)),
            // tokens
            (AbiValue::Token(Tokens::new(123)), "123".to_owned()),
            (
                AbiValue::Token(Tokens::new(max_uint as _)),
                format!("{max_uint}"),
            ),
            (
                AbiValue::Token(Tokens::new(max_uint as u128 + 1)),
                format!("\"{}\"", max_uint + 1),
            ),
        ] {
            let serialized =
                serde_json::to_string(&SerializeAbiValue::with_params(&value, params))?;
            assert_eq!(serialized, json);
            check_parsed(&value, &json);
        }

        Ok(())
    }

    #[test]
    fn fixed_bytes_as_map_key() -> Result<()> {
        let map = AbiValue::map([([0x33u8; 32], 123), ([0x55; 32], 234)]);
        let serialized = map.make_cell(AbiVersion::V2_7)?;
        let parsed = AbiValue::load(
            &map.get_type(),
            AbiVersion::V2_7,
            &mut serialized.as_slice()?,
        )?;
        assert_eq!(map, parsed);

        let map_as_uints = AbiValue::Map(
            PlainAbiType::Uint(256),
            Arc::new(AbiType::Int(32)),
            BTreeMap::from_iter([
                (
                    PlainAbiValue::Uint(256, BigUint::from_bytes_be(&[0x33; 32])),
                    AbiValue::int(32, 123),
                ),
                (
                    PlainAbiValue::Uint(256, BigUint::from_bytes_be(&[0x55; 32])),
                    AbiValue::int(32, 234),
                ),
            ]),
        );
        let serialized_as_uints = map_as_uints.make_cell(AbiVersion::V2_7)?;
        assert_eq!(serialized, serialized_as_uints);

        Ok(())
    }

    #[test]
    fn tuple_tu_from_json() -> Result<()> {
        let json = "{\"a\":\"123\",\"b\":false}";
        let types = [AbiType::int(32).named("a"), AbiType::Bool.named("b")];
        let parsed = NamedAbiValue::tuple_from_json_str(json, &types)?;
        let serialized = serde_json::to_string(&SerializeAbiValues {
            values: &parsed,
            params: Default::default(),
        })?;
        assert_eq!(serialized, json);
        Ok(())
    }
}
