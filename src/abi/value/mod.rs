use std::collections::BTreeMap;
use std::num::NonZeroU8;
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use num_bigint::{BigInt, BigUint};

use super::ty::*;
use super::{IntoAbi, IntoPlainAbi, WithAbiType, WithPlainAbiType, WithoutName};
use crate::abi::error::AbiError;
use crate::cell::{Cell, CellFamily};
use crate::models::{AnyAddr, IntAddr};
use crate::num::Tokens;

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
}

impl PlainAbiValue {
    /// Returns whether this value has the same type as the provided one.
    pub fn has_type(&self, ty: &PlainAbiType) -> bool {
        match (self, ty) {
            (Self::Uint(n, _), PlainAbiType::Uint(t)) => n == t,
            (Self::Int(n, _), PlainAbiType::Int(t)) => n == t,
            (Self::Bool(_), PlainAbiType::Bool) | (Self::Address(_), PlainAbiType::Address) => true,
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
