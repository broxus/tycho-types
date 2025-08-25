use std::collections::BTreeMap;
use std::num::NonZeroU8;

use num_bigint::{BigInt, BigUint, Sign};

use crate::abi::{
    AbiHeader, AbiHeaderType, AbiType, AbiValue, AbiVersion, NamedAbiValue, PlainAbiType,
    PlainAbiValue,
};
use crate::cell::{
    Cell, CellBuilder, CellContext, CellDataBuilder, CellSlice, CellTreeStats, MAX_BIT_LEN,
    MAX_REF_COUNT, Size, Store,
};
use crate::dict::{self, RawDict};
use crate::error::Error;
use crate::models::{AnyAddr, IntAddr};
use crate::num::Tokens;
use crate::prelude::CellFamily;

impl NamedAbiValue {
    /// Tries to store multiple values into a new builder according to the specified ABI version.
    pub fn tuple_to_builder(items: &[Self], version: AbiVersion) -> Result<CellBuilder, Error> {
        let context = Cell::empty_context();
        let mut serializer = AbiSerializer::new(version);
        for item in items {
            serializer.reserve_value(&item.value);
        }
        for item in items {
            ok!(serializer.write_value(&item.value, version, context));
        }
        serializer.finalize(context)
    }

    /// Builds a new cell from multiple values according to the specified ABI version.
    pub fn tuple_to_cell(items: &[Self], version: AbiVersion) -> Result<Cell, Error> {
        Self::tuple_to_builder(items, version).and_then(CellBuilder::build)
    }

    /// Tries to store this value into a new builder according to the specified ABI version.
    pub fn make_builder(&self, version: AbiVersion) -> Result<CellBuilder, Error> {
        self.value.make_builder(version)
    }

    /// Builds a new cell from this value according to the specified ABI version.
    pub fn make_cell(&self, version: AbiVersion) -> Result<Cell, Error> {
        self.value.make_cell(version)
    }
}

impl AbiValue {
    /// Tries to store multiple values into a new builder according to the specified ABI version.
    pub fn tuple_to_builder(values: &[Self], version: AbiVersion) -> Result<CellBuilder, Error> {
        let context = Cell::empty_context();
        let mut serializer = AbiSerializer::new(version);
        for value in values {
            serializer.reserve_value(value);
        }
        for value in values {
            ok!(serializer.write_value(value, version, context));
        }
        serializer.finalize(context)
    }

    /// Builds a new cell from multiple values according to the specified ABI version.
    pub fn tuple_to_cell(values: &[Self], version: AbiVersion) -> Result<Cell, Error> {
        Self::tuple_to_builder(values, version).and_then(CellBuilder::build)
    }

    /// Tries to store this value into a new builder according to the specified ABI version.
    pub fn make_builder(&self, version: AbiVersion) -> Result<CellBuilder, Error> {
        let context = Cell::empty_context();
        let mut serializer = AbiSerializer::new(version);
        serializer.reserve_value(self);
        ok!(serializer.write_value(self, version, context));
        serializer.finalize(context)
    }

    /// Builds a new cell from this value according to the specified ABI version.
    pub fn make_cell(&self, version: AbiVersion) -> Result<Cell, Error> {
        self.make_builder(version).and_then(CellBuilder::build)
    }

    fn compute_size_full(&self, version: AbiVersion) -> CellTreeStats {
        if version.use_max_size() {
            self.compute_max_size_full(version)
        } else {
            self.compute_exact_size_full(version)
        }
    }

    fn compute_max_size_full(&self, abi_version: AbiVersion) -> CellTreeStats {
        if let Self::Tuple(items) = self {
            items
                .iter()
                .map(|item| item.value.compute_max_size_full(abi_version))
                .sum()
        } else {
            self.compute_max_size_short(abi_version).into()
        }
    }

    fn compute_exact_size_full(&self, abi_version: AbiVersion) -> CellTreeStats {
        if let Self::Tuple(items) = self {
            items
                .iter()
                .map(|item| item.value.compute_exact_size_full(abi_version))
                .sum()
        } else {
            self.compute_exact_size_short(abi_version).into()
        }
    }

    fn compute_exact_size_short(&self, abi_version: AbiVersion) -> Size {
        fn compute_varuint_size(n: &NonZeroU8, value: &BigUint) -> Size {
            let value_bytes: u8 = n.get() - 1;
            let len_bits = (8 - value_bytes.leading_zeros()) as u16;
            let value_bits = std::cmp::max(8, 8 * value.bits().div_ceil(8) as u16);
            Size {
                bits: len_bits + value_bits,
                refs: 0,
            }
        }

        match self {
            Self::Uint(n, _) | Self::Int(n, _) => Size { bits: *n, refs: 0 },
            Self::VarUint(n, value) => compute_varuint_size(n, value),
            Self::VarInt(n, value) => compute_varuint_size(n, value.magnitude()),
            Self::Bool(_) => Size { bits: 1, refs: 0 },
            Self::Cell(_)
            | Self::Bytes(_)
            | Self::FixedBytes(_)
            | Self::String(_)
            | Self::Ref(_) => Size { bits: 0, refs: 1 },
            Self::Address(value) => {
                let bit_len = match value.as_ref() {
                    AnyAddr::Var(addr) => addr.bit_len(),
                    AnyAddr::Std(addr) => addr.bit_len(),
                    AnyAddr::Ext(addr) => addr.bit_len(),
                    AnyAddr::None => 2,
                };
                Size {
                    bits: bit_len,
                    refs: 0,
                }
            }
            Self::Token(tokens) => Size {
                bits: tokens.bit_len().unwrap_or(Tokens::MAX_BITS),
                refs: 0,
            },
            Self::Array(_, values) => Size {
                bits: 33,
                refs: !values.is_empty() as u8,
            },
            Self::FixedArray(_, values) => Size {
                bits: 1,
                refs: !values.is_empty() as u8,
            },
            Self::Map(_, _, values) => Size {
                bits: 1,
                refs: !values.is_empty() as u8,
            },
            Self::Optional(ty, value) => {
                if let Some(value) = value {
                    let ty_size = ty.max_size(abi_version);
                    if ty_size.bit_count < MAX_BIT_LEN as u64
                        && ty_size.cell_count < MAX_REF_COUNT as u64
                    {
                        Size { bits: 1, refs: 0 } + value.compute_exact_size_short(abi_version)
                    } else {
                        Size { bits: 1, refs: 1 }
                    }
                } else {
                    Size { bits: 1, refs: 0 }
                }
            }
            Self::Tuple(items) => {
                let mut size = Size::ZERO;
                for item in items {
                    size = size.saturating_add(item.value.compute_exact_size_short(abi_version));
                }
                size
            }
        }
    }

    fn compute_max_size_short(&self, abi_version: AbiVersion) -> Size {
        match self {
            Self::Uint(n, _) | Self::Int(n, _) => Size { bits: *n, refs: 0 },
            Self::VarUint(n, _) | Self::VarInt(n, _) => {
                let value_bytes: u8 = n.get() - 1;
                let bits = (8 - value_bytes.leading_zeros()) as u16 + (value_bytes as u16 * 8);
                Size { bits, refs: 0 }
            }
            Self::Bool(_) => Size { bits: 1, refs: 0 },
            Self::FixedBytes(size) if abi_version >= AbiVersion::V2_4 => Size {
                bits: size.len() as u16 * 8,
                refs: 0,
            },
            Self::Cell(_)
            | Self::Bytes(_)
            | Self::FixedBytes(_)
            | Self::String(_)
            | Self::Ref(_) => Size { bits: 0, refs: 1 },
            Self::Address(_) => Size {
                bits: IntAddr::BITS_MAX,
                refs: 0,
            },
            Self::Token(_) => Size {
                bits: Tokens::MAX_BITS,
                refs: 0,
            },
            Self::Array(..) => Size { bits: 33, refs: 1 },
            Self::FixedArray(..) | Self::Map(..) => Size { bits: 1, refs: 1 },
            Self::Optional(ty, _) => {
                let ty_size = ty.max_size(abi_version);
                if ty_size.bit_count < MAX_BIT_LEN as u64
                    && ty_size.cell_count < MAX_REF_COUNT as u64
                {
                    Size {
                        bits: 1 + ty_size.bit_count as u16,
                        refs: ty_size.cell_count as u8,
                    }
                } else {
                    Size { bits: 1, refs: 1 }
                }
            }
            Self::Tuple(items) => {
                let mut size = Size::ZERO;
                for item in items {
                    size = size.saturating_add(item.value.compute_max_size_short(abi_version));
                }
                size
            }
        }
    }
}

pub(crate) struct AbiSerializer {
    version: AbiVersion,
    current: Size,
    remaining_total: CellTreeStats,
    stack: Vec<CellBuilder>,
}

impl AbiSerializer {
    pub fn new(version: AbiVersion) -> Self {
        Self {
            version,
            current: Size::ZERO,
            remaining_total: CellTreeStats::ZERO,
            stack: Vec::new(),
        }
    }

    pub fn add_offset(&mut self, offset: Size) {
        self.current += offset;
    }

    pub fn reserve_value(&mut self, value: &AbiValue) {
        self.remaining_total += value.compute_size_full(self.version);
    }

    pub fn reserve_headers(&mut self, headers: &[AbiHeaderType], has_public_key: bool) {
        for header in headers {
            self.remaining_total.bit_count += match header {
                AbiHeaderType::Time => 64,
                AbiHeaderType::Expire => 32,
                AbiHeaderType::PublicKey => {
                    1 + if self.version.use_max_size() || has_public_key {
                        256
                    } else {
                        0
                    }
                }
            };
        }
    }

    pub fn finalize(mut self, context: &dyn CellContext) -> Result<CellBuilder, Error> {
        debug_assert_eq!(self.remaining_total, CellTreeStats::ZERO);

        let mut result = self.stack.pop().unwrap_or_default();
        while let Some(builder) = self.stack.pop() {
            let child = ok!(std::mem::replace(&mut result, builder).build_ext(context));
            ok!(result.store_reference(child));
        }
        Ok(result)
    }

    pub fn take_finalize(&mut self, context: &dyn CellContext) -> Result<CellBuilder, Error> {
        debug_assert_eq!(self.remaining_total, CellTreeStats::ZERO);

        self.current = Size::ZERO;
        self.remaining_total = CellTreeStats::ZERO;

        let mut result = self.stack.pop().unwrap_or_default();
        while let Some(builder) = self.stack.pop() {
            let child = ok!(std::mem::replace(&mut result, builder).build_ext(context));
            ok!(result.store_reference(child));
        }
        Ok(result)
    }

    fn begin_child(&self) -> Self {
        Self::new(self.version)
    }

    fn require_builder(&mut self, value_size: Size) -> &mut CellBuilder {
        if self.stack.is_empty() {
            self.stack.push(CellBuilder::new());
        }

        self.remaining_total -= value_size;

        let remaining = Size::MAX - self.current;

        let store_inline = if value_size.bits > remaining.bits || value_size.refs > remaining.refs {
            false
        } else if value_size.refs > 0 && value_size.refs == remaining.refs {
            self.version.major != 1
                && self.remaining_total.cell_count == 0
                && self.remaining_total.bit_count + value_size.bits as u64 <= remaining.bits as u64
        } else {
            true
        };

        if !store_inline {
            self.current = Size::ZERO;
            self.stack.push(CellBuilder::new());
        }

        self.current += value_size;

        // SAFETY: we guarantee that there will be at least one element.
        unsafe { self.stack.last_mut().unwrap_unchecked() }
    }

    pub(crate) fn write_header_value(&mut self, value: &AbiHeader) -> Result<(), Error> {
        match value {
            AbiHeader::Time(value) => self
                .require_builder(Size { bits: 64, refs: 0 })
                .store_u64(*value),
            AbiHeader::Expire(value) => self
                .require_builder(Size { bits: 32, refs: 0 })
                .store_u32(*value),
            AbiHeader::PublicKey(value) => {
                let target = self.require_builder(Size {
                    bits: 1 + if self.version.use_max_size() || value.is_some() {
                        256
                    } else {
                        0
                    },
                    refs: 0,
                });
                match value {
                    None => target.store_bit_zero(),
                    Some(value) => {
                        ok!(target.store_bit_one());
                        target.store_raw(value.as_bytes(), 256)
                    }
                }
            }
        }
    }

    pub(crate) fn write_value(
        &mut self,
        value: &AbiValue,
        abi_version: AbiVersion,
        c: &dyn CellContext,
    ) -> Result<(), Error> {
        match value {
            AbiValue::Uint(n, value) => self.write_uint(*n, value),
            AbiValue::Int(n, value) => self.write_int(*n, value),
            AbiValue::VarUint(n, value) => self.write_varint(*n, Sign::Plus, value),
            AbiValue::VarInt(n, value) => self.write_varint(*n, value.sign(), value.magnitude()),
            AbiValue::Bool(value) => self.write_bool(*value),
            AbiValue::Cell(value) => self.write_cell(value),
            AbiValue::Address(value) => self.write_address(value),
            AbiValue::Bytes(value) => self.write_bytes(value, c),
            AbiValue::FixedBytes(value) => self.write_fixed_bytes(value, abi_version, c),
            AbiValue::String(value) => self.write_bytes(value.as_bytes(), c),
            AbiValue::Token(value) => self.write_tokens(value),
            AbiValue::Tuple(items) => self.write_tuple(items, abi_version, c),
            AbiValue::Array(ty, values) => self.write_array(ty, values, false, abi_version, c),
            AbiValue::FixedArray(ty, values) => self.write_array(ty, values, true, abi_version, c),
            AbiValue::Map(k, v, values) => self.write_map(k, v, values, abi_version, c),
            AbiValue::Optional(ty, value) => {
                self.write_optional(ty, value.as_deref(), abi_version, c)
            }
            AbiValue::Ref(value) => self.write_ref(value, abi_version, c),
        }
    }

    pub(crate) fn write_tuple(
        &mut self,
        items: &[NamedAbiValue],
        abi_version: AbiVersion,
        context: &dyn CellContext,
    ) -> Result<(), Error> {
        for item in items {
            ok!(self.write_value(&item.value, abi_version, context));
        }
        Ok(())
    }

    fn write_uint(&mut self, bits: u16, value: &BigUint) -> Result<(), Error> {
        let target = self.require_builder(Size { bits, refs: 0 });
        target.store_biguint(value, bits, false)
    }

    fn write_int(&mut self, bits: u16, value: &BigInt) -> Result<(), Error> {
        let target = self.require_builder(Size { bits, refs: 0 });
        target.store_bigint(value, bits, true)
    }

    fn write_varint(&mut self, size: NonZeroU8, sign: Sign, value: &BigUint) -> Result<(), Error> {
        let is_negative = sign == Sign::Minus;
        let bytes = to_signed_bytes_be(is_negative, value);

        let max_value_size = size.get() - 1;
        if bytes.len() > max_value_size as usize {
            return Err(Error::IntOverflow);
        }

        let len_bits = (8 - max_value_size.leading_zeros()) as u16;
        let value_bits = (bytes.len() * 8) as u16;

        let target = self.require_builder(Size {
            bits: if self.version.use_max_size() {
                len_bits + (max_value_size as u16) * 8
            } else {
                len_bits + value_bits
            },
            refs: 0,
        });

        ok!(target.store_small_uint(bytes.len() as u8, len_bits));
        target.store_raw(&bytes, value_bits)
    }

    fn write_bool(&mut self, value: bool) -> Result<(), Error> {
        let target = self.require_builder(Size { bits: 1, refs: 0 });
        target.store_bit(value)
    }

    fn write_cell(&mut self, cell: &Cell) -> Result<(), Error> {
        let target = self.require_builder(Size { bits: 0, refs: 1 });
        target.store_reference(cell.clone())
    }

    fn write_address(&mut self, address: &AnyAddr) -> Result<(), Error> {
        let target = self.require_builder(Size {
            bits: if self.version.use_max_size() {
                IntAddr::BITS_MAX
            } else {
                address.bit_len()
            },
            refs: 0,
        });
        address.store_into(target, Cell::empty_context())
    }

    fn write_tokens(&mut self, tokens: &Tokens) -> Result<(), Error> {
        let target = self.require_builder(Size {
            bits: if self.version.use_max_size() {
                Tokens::MAX_BITS
            } else {
                tokens.bit_len().unwrap_or(Tokens::MAX_BITS)
            },
            refs: 0,
        });
        tokens.store_into(target, Cell::empty_context())
    }

    fn write_fixed_bytes(
        &mut self,
        data: &[u8],
        abi_version: AbiVersion,
        context: &dyn CellContext,
    ) -> Result<(), Error> {
        if abi_version >= AbiVersion::V2_4 {
            let target = self.require_builder(Size {
                bits: data.len() as u16 * 8,
                refs: 0,
            });
            target.store_raw(data, data.len() as u16 * 8)
        } else {
            self.write_bytes(data, context)
        }
    }

    fn write_bytes(&mut self, mut data: &[u8], context: &dyn CellContext) -> Result<(), Error> {
        const MAX_BYTES_PER_BUILDER: usize = (MAX_BIT_LEN / 8) as usize;
        let mut len = data.len();

        let mut bytes_per_builder = if self.version.major > 1 {
            match len % MAX_BYTES_PER_BUILDER {
                0 => MAX_BYTES_PER_BUILDER,
                x => x,
            }
        } else {
            std::cmp::min(MAX_BYTES_PER_BUILDER, len)
        };

        let mut result = CellBuilder::new();
        while len > 0 {
            len -= bytes_per_builder;
            let (head, tail) = data.split_at(len);
            data = head;

            if result.size_bits() > 0 {
                let child = ok!(std::mem::take(&mut result).build_ext(context));
                ok!(result.store_reference(child));
            }

            ok!(result.store_raw(tail, (bytes_per_builder * 8) as u16));

            bytes_per_builder = std::cmp::min(MAX_BYTES_PER_BUILDER, len);
        }

        let target = self.require_builder(Size { bits: 0, refs: 1 });
        target.store_reference(ok!(result.build()))
    }

    fn write_array(
        &mut self,
        value_ty: &AbiType,
        values: &[AbiValue],
        fixed_len: bool,
        abi_version: AbiVersion,
        context: &dyn CellContext,
    ) -> Result<(), Error> {
        let inline_value = fits_into_dict_leaf(32, value_ty.max_bits(abi_version));

        let mut dict = RawDict::<32>::new();
        let mut key_builder = CellDataBuilder::new();
        let mut serializer = self.begin_child();
        for (i, value) in values.iter().enumerate() {
            ok!(key_builder.store_u32(i as u32));

            let value = {
                serializer.reserve_value(value);
                ok!(serializer.write_value(value, abi_version, context));
                ok!(serializer.take_finalize(context))
            };
            let value = if inline_value {
                InlineOrRef::Inline(value.as_full_slice())
            } else {
                InlineOrRef::Ref(ok!(value.build_ext(context)))
            };

            ok!(dict.set(key_builder.as_data_slice(), value));

            key_builder.clear_bits();
        }

        let bits = 1 + if fixed_len { 1 } else { 32 };
        let refs = if self.version.use_max_size() {
            1
        } else {
            !values.is_empty() as u8
        };

        let target = self.require_builder(Size { bits, refs });

        if !fixed_len {
            ok!(target.store_u32(values.len() as u32));
        }
        dict.store_into(target, context)
    }

    fn write_map(
        &mut self,
        key_ty: &PlainAbiType,
        value_ty: &AbiType,
        value: &BTreeMap<PlainAbiValue, AbiValue>,
        abi_version: AbiVersion,
        context: &dyn CellContext,
    ) -> Result<(), Error> {
        let key_bits = key_ty.key_bits();
        let inline_value = fits_into_dict_leaf(key_bits, value_ty.max_bits(abi_version));

        let mut dict = None::<Cell>;
        let mut key_builder = CellBuilder::new();
        let mut serializer = self.begin_child();
        for (key, value) in value {
            ok!(key.store_into(&mut key_builder, context));

            let value = {
                serializer.reserve_value(value);
                ok!(serializer.write_value(value, abi_version, context));
                ok!(serializer.take_finalize(context))
            };
            let value = if inline_value {
                InlineOrRef::Inline(value.as_full_slice())
            } else {
                InlineOrRef::Ref(ok!(value.build_ext(context)))
            };

            ok!(dict::dict_insert(
                &mut dict,
                &mut key_builder.as_data_slice(),
                key_bits,
                &value,
                dict::SetMode::Set,
                context,
            ));

            key_builder.clear_bits();
        }

        let target = self.require_builder(Size { bits: 1, refs: 1 });
        dict.store_into(target, context)
    }

    fn write_optional(
        &mut self,
        ty: &AbiType,
        value: Option<&AbiValue>,
        abi_version: AbiVersion,
        context: &dyn CellContext,
    ) -> Result<(), Error> {
        let (max_size, inline) = {
            let ty_size = ty.max_size(abi_version);
            if ty_size.bit_count < MAX_BIT_LEN as _ && ty_size.cell_count < MAX_REF_COUNT as _ {
                let size = Size {
                    bits: 1 + ty_size.bit_count as u16,
                    refs: ty_size.cell_count as u8,
                };
                (size, true)
            } else {
                (Size { bits: 1, refs: 1 }, false)
            }
        };

        match value {
            Some(value) => {
                let value = {
                    let mut serializer = self.begin_child();
                    serializer.reserve_value(value);
                    ok!(serializer.write_value(value, abi_version, context));
                    ok!(serializer.finalize(context))
                };

                let target = self.require_builder(if self.version.use_max_size() {
                    max_size
                } else if inline {
                    Size {
                        bits: 1 + value.size_bits(),
                        refs: value.size_refs(),
                    }
                } else {
                    Size { bits: 1, refs: 1 }
                });

                ok!(target.store_bit(true));
                if inline {
                    target.store_builder(&value)
                } else {
                    target.store_reference(ok!(value.build_ext(context)))
                }
            }
            None => {
                let target = self.require_builder(if self.version.use_max_size() {
                    max_size
                } else {
                    Size { bits: 1, refs: 0 }
                });
                target.store_bit(false)
            }
        }
    }

    fn write_ref(
        &mut self,
        value: &AbiValue,
        abi_version: AbiVersion,
        context: &dyn CellContext,
    ) -> Result<(), Error> {
        let cell = {
            let mut serializer = self.begin_child();
            serializer.reserve_value(value);
            ok!(serializer.write_value(value, abi_version, context));
            let builder = ok!(serializer.finalize(context));
            ok!(builder.build_ext(context))
        };
        let target = self.require_builder(Size { bits: 0, refs: 1 });
        target.store_reference(cell)
    }
}

fn to_signed_bytes_be(is_negative: bool, value: &BigUint) -> Vec<u8> {
    #[inline]
    fn is_zero(value: &u8) -> bool {
        *value == 0
    }

    #[inline]
    pub fn twos_complement_le(digits: &mut [u8]) {
        let mut carry = true;
        for digit in digits {
            *digit = !*digit;
            if carry {
                let (d, c) = digit.overflowing_add(1);
                *digit = d;
                carry = c;
            }
        }
    }

    fn negative_to_signed_bytes_be(value: &BigUint) -> Vec<u8> {
        let mut bytes = value.to_bytes_le();
        let last_byte = bytes.last().cloned().unwrap_or(0);
        if last_byte > 0x7f && !(last_byte == 0x80 && bytes.iter().rev().skip(1).all(is_zero)) {
            // msb used by magnitude, extend by 1 byte
            bytes.push(0);
        }
        twos_complement_le(&mut bytes);
        bytes.reverse();
        bytes
    }

    if is_negative {
        negative_to_signed_bytes_be(value)
    } else {
        value.to_bytes_be()
    }
}

const fn fits_into_dict_leaf(key_bits: u16, value_bits: usize) -> bool {
    // NOTE: Label offset is calculated as 2 bits for tag + max ceil(log2(key_bits)).
    // However this might be incorrect as we have other parts of the label to store.
    const LABEL_OFFSET: usize = 12;

    LABEL_OFFSET + key_bits as usize + value_bits <= MAX_BIT_LEN as usize
}

enum InlineOrRef<'a> {
    Inline(CellSlice<'a>),
    Ref(Cell),
}

impl Store for InlineOrRef<'_> {
    fn store_into(&self, builder: &mut CellBuilder, _: &dyn CellContext) -> Result<(), Error> {
        match self {
            Self::Inline(slice) => builder.store_slice(slice),
            Self::Ref(cell) => builder.store_reference(cell.clone()),
        }
    }
}

impl Store for PlainAbiValue {
    fn store_into(&self, builder: &mut CellBuilder, f: &dyn CellContext) -> Result<(), Error> {
        match self {
            Self::Uint(bits, value) => builder.store_biguint(value, *bits, false),
            Self::Int(bits, value) => builder.store_bigint(value, *bits, true),
            Self::Bool(bit) => builder.store_bit(*bit),
            Self::Address(address) => address.store_into(builder, f),
        }
    }
}

impl AbiVersion {
    fn use_max_size(&self) -> bool {
        self >= &Self::V2_2
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::Bytes;

    use super::*;
    use crate::dict::Dict;
    use crate::models::{StdAddr, VarAddr};
    use crate::num::Uint9;
    use crate::prelude::{CellFamily, HashBytes};

    const VX_X: [AbiVersion; 5] = [
        AbiVersion::V1_0,
        AbiVersion::V2_0,
        AbiVersion::V2_1,
        AbiVersion::V2_2,
        AbiVersion::V2_3,
    ];
    const V2_X: [AbiVersion; 4] = [
        AbiVersion::V2_0,
        AbiVersion::V2_1,
        AbiVersion::V2_2,
        AbiVersion::V2_3,
    ];

    fn check_encoding(version: AbiVersion, value: AbiValue) {
        let cell = value.make_cell(version).unwrap();
        let parsed =
            AbiValue::load(&value.get_type(), version, &mut cell.as_slice().unwrap()).unwrap();
        assert_eq!(value, parsed);
    }

    #[test]
    fn int_overflow() {
        assert_eq!(
            AbiValue::Uint(16, BigUint::from(u32::MAX)).make_cell(AbiVersion::V2_2),
            Err(Error::IntOverflow)
        );

        assert_eq!(
            AbiValue::Uint(16, BigUint::from(u16::MAX as u32 + 1)).make_cell(AbiVersion::V2_2),
            Err(Error::IntOverflow)
        );
    }

    #[test]
    fn encode_int() {
        macro_rules! define_tests {
            ($v:ident, { $($abi:ident($bits:literal) => [$($expr:expr),*$(,)?]),*$(,)? }) => {$(
                $(check_encoding($v, AbiValue::$abi($bits, $expr));)*
            )*};
        }

        for v in VX_X {
            println!("ABIv{v}");
            define_tests!(v, {
                uint(8) => [0u8, 123u8, u8::MAX],
                uint(16) => [0u16, 1234u16, u16::MAX],
                uint(32) => [0u32, 123456u32, u32::MAX],
                uint(64) => [0u64, 123456789u64, u64::MAX],
                uint(128) => [0u128, 123456789123123123123u128, u128::MAX],

                int(8) => [0i8, 123i8, i8::MIN, i8::MAX],
                int(16) => [0i16, 1234i16, i16::MIN, i16::MAX],
                int(32) => [0i32, 123456i32, i32::MIN, i32::MAX],
                int(64) => [0i64, 123456789i64, i64::MIN, i64::MAX],
                int(128) => [0i128, 123456789123123123123i128, i128::MIN, i128::MAX],
            });
        }
    }

    #[test]
    fn encode_varint() {
        for v in V2_X {
            println!("ABIv{v}");
            check_encoding(v, AbiValue::varuint(4, 0u32));
            check_encoding(v, AbiValue::varuint(4, u32::MAX >> 8));
            check_encoding(v, AbiValue::varuint(4, 123321u32));

            check_encoding(v, AbiValue::varuint(8, 0u32));
            check_encoding(v, AbiValue::varuint(8, u64::MAX >> 8));
            check_encoding(v, AbiValue::varuint(8, 1233213213123123u64));

            check_encoding(v, AbiValue::varuint(16, 0u8));
            check_encoding(v, AbiValue::Token(Tokens::ZERO));

            let mut prev_value = 0;
            for byte in 0..15 {
                let value = (0xffu128 << (byte * 8)) | prev_value;
                prev_value = value;
                check_encoding(v, AbiValue::varuint(16, value));
                check_encoding(v, AbiValue::Token(Tokens::new(value)));
            }

            check_encoding(v, AbiValue::varuint(128, 0u8));
            check_encoding(v, AbiValue::varuint(128, BigUint::from(1u8) << (126 * 8)));
        }
    }

    #[test]
    fn encode_bool() {
        for v in VX_X {
            println!("ABIv{v}");
            check_encoding(v, AbiValue::Bool(false));
            check_encoding(v, AbiValue::Bool(true));
        }
    }

    #[test]
    fn encode_cell() {
        // Simple cases
        for v in VX_X {
            check_encoding(v, AbiValue::Cell(Cell::empty_cell()));
        }

        // Complex case
        let value = AbiValue::unnamed_tuple([
            AbiValue::Cell(Cell::empty_cell()),
            AbiValue::Cell(Cell::empty_cell()),
            AbiValue::Cell(Cell::empty_cell()),
            AbiValue::Cell(Cell::empty_cell()),
        ]);

        // ABI v1
        let cell_v1 = {
            let mut builder = CellBuilder::new();
            builder.store_reference(Cell::empty_cell()).unwrap();
            builder.store_reference(Cell::empty_cell()).unwrap();
            builder.store_reference(Cell::empty_cell()).unwrap();
            builder
                .store_reference(CellBuilder::build_from(Cell::empty_cell()).unwrap())
                .unwrap();
            builder.build().unwrap()
        };

        assert_eq!(value.make_cell(AbiVersion::V1_0).unwrap(), cell_v1);

        // ABI v2.x
        let cell_v2 = CellBuilder::build_from((
            Cell::empty_cell(),
            Cell::empty_cell(),
            Cell::empty_cell(),
            Cell::empty_cell(),
        ))
        .unwrap();

        for v in V2_X {
            println!("ABIv{v}");
            assert_eq!(value.make_cell(v).unwrap(), cell_v2);
        }
    }

    #[test]
    fn encode_address() {
        for v in VX_X {
            check_encoding(v, AbiValue::address(StdAddr::new(0, HashBytes([0xff; 32]))));
            check_encoding(
                v,
                AbiValue::address(VarAddr {
                    address_len: Uint9::new(10 * 8),
                    anycast: None,
                    workchain: 123456,
                    address: vec![0xffu8; 10],
                }),
            );
        }
    }

    #[test]
    fn encode_bytes() {
        let mut bytes = vec![0xffu8; 256];
        bytes[0] = 0xff; // mark start
        let bytes = Bytes::from(bytes);

        let empty_bytes_cell = {
            let mut builder = CellBuilder::new();
            builder.store_reference(Cell::empty_cell()).unwrap();
            builder.build().unwrap()
        };

        // Check simple encoding
        for v in VX_X {
            println!("ABIv{v}");

            check_encoding(v, AbiValue::Bytes(bytes.clone()));

            // Bytes must have the same encoding as fixedbytes
            assert_eq!(
                AbiValue::Bytes(bytes.clone()).make_cell(v),
                AbiValue::FixedBytes(bytes.clone()).make_cell(v)
            );

            assert_eq!(
                AbiValue::Bytes(Bytes::new()).make_cell(v).unwrap(),
                empty_bytes_cell
            );
        }

        // ABI v1
        let cell_v1 = CellBuilder::build_from({
            let mut builder = CellBuilder::new();
            builder.store_raw(&[0xff; 2], 2 * 8).unwrap();

            builder
                .store_reference({
                    let mut builder = CellBuilder::new();
                    builder.store_raw(&[0xff; 127], 127 * 8).unwrap();

                    builder
                        .store_reference({
                            let mut builder = CellBuilder::new();
                            builder.store_raw(&[0xff; 127], 127 * 8).unwrap();
                            builder.build().unwrap()
                        })
                        .unwrap();

                    builder.build().unwrap()
                })
                .unwrap();

            builder.build().unwrap()
        })
        .unwrap();

        // ABI v2
        let cell_v2 = CellBuilder::build_from({
            let mut builder = CellBuilder::new();
            builder.store_raw(&[0xff; 127], 127 * 8).unwrap();

            builder
                .store_reference({
                    let mut builder = CellBuilder::new();
                    builder.store_raw(&[0xff; 127], 127 * 8).unwrap();

                    builder
                        .store_reference({
                            let mut builder = CellBuilder::new();
                            builder.store_raw(&[0xff; 2], 2 * 8).unwrap();
                            builder.build().unwrap()
                        })
                        .unwrap();

                    builder.build().unwrap()
                })
                .unwrap();

            builder.build().unwrap()
        })
        .unwrap();

        //

        assert_eq!(
            AbiValue::Bytes(bytes.clone())
                .make_cell(AbiVersion::V1_0)
                .unwrap(),
            cell_v1
        );

        for v in V2_X {
            println!("ABIv{v}");

            assert_eq!(
                AbiValue::Bytes(bytes.clone()).make_cell(v).unwrap(),
                cell_v2
            );
        }
    }

    #[test]
    fn encode_string() {
        for v in VX_X {
            println!("ABIv{v}");
            check_encoding(v, AbiValue::String(String::new()));
            check_encoding(v, AbiValue::String("Hello".to_owned()));
            check_encoding(
                v,
                AbiValue::String(
                    std::iter::repeat_with(|| "HELLO".to_owned())
                        .take(200)
                        .collect::<Vec<String>>()
                        .join(" "),
                ),
            );
        }
    }

    #[test]
    fn encode_nested_simple_tuple() {
        let value = AbiValue::unnamed_tuple([
            AbiValue::uint(32, 0u32),
            AbiValue::Cell(Cell::empty_cell()),
            AbiValue::Bool(false),
            AbiValue::unnamed_tuple([
                AbiValue::int(8, -15),
                AbiValue::int(16, -9845),
                AbiValue::unnamed_tuple([
                    AbiValue::int(32, -1),
                    AbiValue::int(64, 12345678),
                    AbiValue::int(128, -12345678),
                ]),
            ]),
            AbiValue::unnamed_tuple([
                AbiValue::uint(8, 255_u8),
                AbiValue::uint(16, 0_u16),
                AbiValue::unnamed_tuple([
                    AbiValue::uint(32, 256_u32),
                    AbiValue::uint(64, 123_u64),
                    AbiValue::uint(128, 1234567890_u128),
                ]),
            ]),
        ]);

        for v in VX_X {
            println!("ABIv{v}");

            let cell = value.make_cell(v).unwrap();
            assert_eq!(
                cell.bit_len(),
                32 + 1 + 8 + 16 + 32 + 64 + 128 + 8 + 16 + 32 + 64 + 128
            );
            assert_eq!(cell.reference_count(), 1);

            check_encoding(v, value.clone());
        }
    }

    #[test]
    fn encode_tuple_four_refs_and_four_uint256() {
        let bytes = HashBytes([0xff; 32]);
        let bytes_cell = CellBuilder::build_from(bytes).unwrap();

        let cell = {
            let mut builder = CellBuilder::new();
            builder.store_u32(0).unwrap();
            builder.store_reference(Cell::empty_cell()).unwrap();

            builder.store_reference(bytes_cell.clone()).unwrap();
            builder.store_reference(bytes_cell.clone()).unwrap();

            let mut second_builder = CellBuilder::new();
            second_builder.store_reference(bytes_cell.clone()).unwrap();
            second_builder.store_u256(&bytes).unwrap();
            second_builder.store_u256(&bytes).unwrap();
            second_builder.store_u256(&bytes).unwrap();

            let mut third_builder = CellBuilder::new();
            third_builder.store_u256(&bytes).unwrap();

            second_builder
                .store_reference(third_builder.build().unwrap())
                .unwrap();
            builder
                .store_reference(second_builder.build().unwrap())
                .unwrap();

            builder.build().unwrap()
        };

        let value = AbiValue::unnamed_tuple([
            AbiValue::uint(32, 0_u32),
            AbiValue::Cell(Cell::empty_cell()),
            AbiValue::Cell(bytes_cell.clone()),
            AbiValue::Bytes(Bytes::copy_from_slice(bytes.as_slice())),
            AbiValue::Cell(bytes_cell),
            AbiValue::uint(256, BigUint::from_bytes_be(bytes.as_slice())),
            AbiValue::uint(256, BigUint::from_bytes_be(bytes.as_slice())),
            AbiValue::uint(256, BigUint::from_bytes_be(bytes.as_slice())),
            AbiValue::uint(256, BigUint::from_bytes_be(bytes.as_slice())),
        ]);

        for v in VX_X {
            println!("ABIv{v}");

            assert_eq!(value.make_cell(v).unwrap(), cell);
            check_encoding(v, value.clone());
        }
    }

    #[test]
    fn encode_tuple_four_refs_and_one_uint256() {
        let bytes = HashBytes([0x55; 32]);
        let bytes_cell = CellBuilder::build_from(bytes).unwrap();

        let mut builder = CellBuilder::new();
        builder.store_u32(0).unwrap();
        builder.store_reference(Cell::empty_cell()).unwrap();

        builder.store_reference(bytes_cell.clone()).unwrap();
        builder.store_reference(bytes_cell.clone()).unwrap();

        let cell_v2 = {
            let mut builder = builder.clone();
            builder.store_reference(bytes_cell.clone()).unwrap();
            builder.store_u256(&bytes).unwrap();
            builder.build().unwrap()
        };

        let cell_v1 = {
            let mut child_builder = CellBuilder::new();
            child_builder.store_reference(bytes_cell.clone()).unwrap();
            child_builder.store_u256(&bytes).unwrap();

            builder
                .store_reference(child_builder.build().unwrap())
                .unwrap();
            builder.build().unwrap()
        };

        let value = AbiValue::unnamed_tuple([
            AbiValue::uint(32, 0_u32),
            AbiValue::Cell(Cell::empty_cell()),
            AbiValue::Cell(bytes_cell.clone()),
            AbiValue::Bytes(Bytes::copy_from_slice(bytes.as_slice())),
            AbiValue::Cell(bytes_cell.clone()),
            AbiValue::uint(256, BigUint::from_bytes_be(bytes.as_slice())),
        ]);

        for v in VX_X {
            println!("ABIv{v}");
            check_encoding(v, value.clone());
        }

        assert_eq!(value.make_cell(AbiVersion::V1_0).unwrap(), cell_v1);

        for v in V2_X {
            println!("ABIv{v}");
            assert_eq!(value.make_cell(v).unwrap(), cell_v2);
        }
    }

    #[test]
    fn encode_map_simple() {
        let bytes = HashBytes([0x55; 32]);
        let bytes_cell = CellBuilder::build_from(bytes).unwrap();

        let mut bytes_map = Dict::<u8, Cell>::new();
        for i in 1..=3 {
            bytes_map.set(i, bytes_cell.clone()).unwrap();
        }
        let bytes_map_cell = CellBuilder::build_from(bytes_map).unwrap();
        let bytes_map_value = AbiValue::map([
            (1u8, Bytes::copy_from_slice(bytes.as_slice())),
            (2, Bytes::copy_from_slice(bytes.as_slice())),
            (3, Bytes::copy_from_slice(bytes.as_slice())),
        ]);

        for v in VX_X {
            println!("ABIv{v}");

            assert_eq!(bytes_map_value.make_cell(v).unwrap(), bytes_map_cell);
            check_encoding(v, bytes_map_value.clone());
        }

        //
        let mut int_map = Dict::<i16, i128>::new();
        for i in -1..=1 {
            int_map.set(i, i as i128).unwrap();
        }
        let int_map_cell = CellBuilder::build_from(int_map).unwrap();
        let int_map_value = AbiValue::map([(-1i16, -1i128), (0, 0), (1, 1)]);

        for v in VX_X {
            println!("ABIv{v}");

            assert_eq!(int_map_value.make_cell(v).unwrap(), int_map_cell);
            check_encoding(v, int_map_value.clone());
        }

        //
        let mut tuples_map = Dict::<u128, (u32, bool)>::new();
        for i in 1..=5 {
            tuples_map.set(i as u128, (i, i % 2 != 0)).unwrap();
        }
        let tuples_map_cell = CellBuilder::build_from(tuples_map).unwrap();
        let tuples_map_value = AbiValue::map([
            (1u128, (1u32, true)),
            (2, (2, false)),
            (3, (3, true)),
            (4, (4, false)),
            (5, (5, true)),
        ]);

        for v in VX_X {
            println!("ABIv{v}");

            assert_eq!(tuples_map_value.make_cell(v).unwrap(), tuples_map_cell);
            check_encoding(v, tuples_map_value.clone());
        }

        //
        let addr1 = StdAddr::new(0, HashBytes([0x11; 32]));
        let addr2 = StdAddr::new(0, HashBytes([0x22; 32]));

        let mut addr_map = Dict::<StdAddr, u32>::new();
        addr_map.set(&addr1, 123).unwrap();
        addr_map.set(&addr2, 456).unwrap();
        let addr_map_cell = CellBuilder::build_from(addr_map).unwrap();
        let addr_map_value = AbiValue::map([(addr1, 123u32), (addr2, 456)]);

        for v in VX_X {
            println!("ABIv{v}");

            assert_eq!(addr_map_value.make_cell(v).unwrap(), addr_map_cell);
            check_encoding(v, addr_map_value.clone());
        }
    }

    #[test]
    fn encode_map_big_value() {
        //
        let tuple_value = AbiValue::unnamed_tuple([
            AbiValue::uint(256, 1_u32),
            AbiValue::uint(256, 2_u32),
            AbiValue::uint(256, 3_u32),
            AbiValue::uint(256, 4_u32),
        ]);

        //
        let mut map_value = CellBuilder::new();
        map_value.store_u128(0).unwrap();
        map_value.store_u128(1).unwrap();
        map_value.store_u128(0).unwrap();
        map_value.store_u128(2).unwrap();
        map_value.store_u128(0).unwrap();
        map_value.store_u128(3).unwrap();
        map_value
            .store_reference(CellBuilder::build_from((0u128, 4u128)).unwrap())
            .unwrap();
        let map_value = map_value.build().unwrap();

        let mut key = CellBuilder::new();
        key.store_u128(0).unwrap();
        key.store_u128(123).unwrap();

        let mut map = RawDict::<256>::new();
        map.set(key.as_data_slice(), &map_value).unwrap();
        let map_cell = CellBuilder::build_from(map).unwrap();
        let map = AbiValue::Map(
            PlainAbiType::Uint(256),
            Arc::new(tuple_value.get_type()),
            BTreeMap::from([(
                PlainAbiValue::Uint(256, BigUint::from(123u32)),
                tuple_value.clone(),
            )]),
        );

        for v in VX_X {
            println!("ABIv{v}");
            assert_eq!(map.make_cell(v).unwrap(), map_cell);
            check_encoding(v, map.clone());
        }

        //
        let mut key = CellBuilder::new();
        key.store_u32(0).unwrap();

        let mut array = RawDict::<32>::new();
        array.set(key.as_data_slice(), &map_value).unwrap();
        let array_cell = CellBuilder::build_from((1u32, array)).unwrap();
        let array = AbiValue::Array(Arc::new(tuple_value.get_type()), vec![tuple_value]);

        for v in V2_X {
            println!("ABIv{v}");
            assert_eq!(array.make_cell(v).unwrap(), array_cell);
            check_encoding(v, array.clone());
        }
    }

    #[test]
    fn encode_optional() {
        const STR: &str = "Some string";

        let string_cell = {
            let mut builder = CellBuilder::new();
            builder
                .store_raw(STR.as_bytes(), (STR.len() * 8) as u16)
                .unwrap();
            builder.build().unwrap()
        };
        let string_value = AbiValue::String(STR.to_owned());

        let tuple_value = AbiValue::unnamed_tuple([
            string_value.clone(),
            string_value.clone(),
            string_value.clone(),
            string_value.clone(),
        ]);

        let value = AbiValue::unnamed_tuple([
            AbiValue::uint(32, 0u32),
            AbiValue::Cell(Cell::empty_cell()),
            AbiValue::varint(16, -123),
            AbiValue::varuint(32, 456u32),
            AbiValue::optional(None::<bool>),
            AbiValue::Optional(
                Arc::new(AbiType::Uint(1022)),
                Some(Box::new(AbiValue::uint(1022, 1u32))),
            ),
            AbiValue::Optional(
                Arc::new(AbiType::varuint(128)),
                Some(Box::new(AbiValue::varuint(128, 123u32))),
            ),
            AbiValue::Optional(
                Arc::new(tuple_value.get_type()),
                Some(Box::new(tuple_value)),
            ),
        ]);

        let cell = {
            let mut builder = CellBuilder::new();
            builder.store_u32(0).unwrap();
            builder.store_reference(Cell::empty_cell()).unwrap();

            builder.store_small_uint(1, 4).unwrap();
            builder.store_u8(-123i8 as _).unwrap();

            builder.store_small_uint(2, 5).unwrap();
            builder.store_u16(456).unwrap();

            builder.store_bit_zero().unwrap();

            builder
                .store_reference({
                    let mut builder = CellBuilder::new();
                    builder.store_bit_one().unwrap();
                    builder.store_zeros(127 * 8).unwrap();
                    builder.store_small_uint(1, 6).unwrap();

                    builder
                        .store_reference({
                            let mut builder = CellBuilder::new();
                            builder.store_bit_one().unwrap();
                            builder
                                .store_reference({
                                    let mut builder = CellBuilder::new();
                                    builder.store_small_uint(1, 7).unwrap();
                                    builder.store_u8(123).unwrap();
                                    builder.build().unwrap()
                                })
                                .unwrap();

                            builder.store_bit_one().unwrap();
                            builder
                                .store_reference(
                                    CellBuilder::build_from((
                                        string_cell.clone(),
                                        string_cell.clone(),
                                        string_cell.clone(),
                                        string_cell.clone(),
                                    ))
                                    .unwrap(),
                                )
                                .unwrap();

                            builder.build().unwrap()
                        })
                        .unwrap();

                    builder.build().unwrap()
                })
                .unwrap();

            builder.build().unwrap()
        };

        for v in V2_X {
            println!("ABIv{v}");

            assert_eq!(value.make_cell(v).unwrap(), cell);
            check_encoding(v, value.clone());
        }
    }

    #[test]
    fn encode_ref() {
        let cell = CellBuilder::build_from((
            CellBuilder::build_from(123u64).unwrap(),
            CellBuilder::build_from((true, Cell::empty_cell())).unwrap(),
        ))
        .unwrap();

        let value = AbiValue::unnamed_tuple([
            AbiValue::reference(123u64),
            AbiValue::reference((true, Cell::empty_cell())),
        ]);

        for v in V2_X {
            println!("ABIv{v}");
            assert_eq!(value.make_cell(v).unwrap(), cell);
            check_encoding(v, value.clone());
        }
    }
}
