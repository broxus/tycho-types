//! ABI related error types.

use std::sync::Arc;

/// Error type for ABI version parsing related errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ParseAbiVersionError {
    /// Expected a dot separated major and minor components.
    #[error("invalid format")]
    InvalidFormat,
    /// Failed to parse version component as number.
    #[error("invalid component")]
    InvalidComponent(#[source] std::num::ParseIntError),
}

/// Error type for ABI type parsing related errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ParseAbiTypeError {
    /// Error while parsing array type.
    #[error("invalid array type")]
    InvalidArrayType,
    /// Error while parsing array length.
    #[error("invalid array length")]
    InvalidArrayLength(#[source] std::num::ParseIntError),
    /// Error while parsing integer bit length.
    #[error("invalid integer bit length")]
    InvalidBitLen(#[source] std::num::ParseIntError),
    /// Error while parsing varint or fixedbytes byte length.
    #[error("invalid byte length")]
    InvalidByteLen(#[source] std::num::ParseIntError),
    /// Expected type to be terminated with ')'.
    #[error("unterminated inner type")]
    UnterminatedInnerType,
    /// Expected value type for map.
    #[error("map value type not found")]
    ValueTypeNotFound,
    /// Invalid ABI type.
    #[error("unknown type")]
    UnknownType,
}

/// Error type for named ABI type parsing related errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ParseNamedAbiTypeError {
    /// Error while parsing ABI type.
    #[error("invalid type `{ty}`: {error}")]
    InvalidType {
        /// Parsed ABI type.
        ty: Box<str>,
        /// ABI type parsing error.
        #[source]
        error: ParseAbiTypeError,
    },
    /// Components array was not expected.
    #[error("unexpected components for `{ty}`")]
    UnexpectedComponents {
        /// Parsed ABI type.
        ty: Box<str>,
    },
    /// Expected components array for tuple.
    #[error("expected components for `{ty}`")]
    ExpectedComponents {
        /// Parsed ABI type.
        ty: Box<str>,
    },
}

/// Error type for ABI values unpacking related errors.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum AbiError {
    /// Expected a different value type.
    #[error("expected ABI type `{expected}`, got `{ty}`")]
    TypeMismatch {
        /// A full signature of the expected type.
        expected: Box<str>,
        /// A full signature of the received type.
        ty: Box<str>,
    },
    /// More parameters were passed into state init than needed.
    #[error("unexpected init data parameter `{0}`")]
    UnexpectedInitDataParam(Arc<str>),
    /// Field is absent in the init data dictionary.
    #[error("missing init data field `{0}`")]
    InitDataFieldNotFound(Arc<str>),
    /// There are still some unconsumed bits or refs and we did not expect this.
    #[error("slice was not fully consumed during parsing")]
    IncompleteDeserialization,
    /// All cells for `bytes` or `fixedbytes` must have data multiple of 8.
    #[error("number of bits in a cell is not a multiple of 8")]
    ExpectedCellWithBytes,
    /// Expected a valid utf8-encoded string.
    #[error("invalid string")]
    InvalidString(#[from] std::str::Utf8Error),
    /// Invalid length for a fixedarrayN.
    #[error("expected array of len {expected}, got {len}")]
    ArraySizeMismatch {
        /// `N` in `fixedarrayN`.
        expected: usize,
        /// Length of the parsed array.
        len: usize,
    },
    /// Invalid length for a fixedbytes{n}.
    #[error("expected bytes of len {expected}, got {len}")]
    BytesSizeMismatch {
        /// `N` in `fixedbytesN`.
        expected: usize,
        /// Length of the parsed bytes.
        len: usize,
    },
    /// Address is required for signature for some ABI versions and it was not provided.
    #[error("an address was expected for signing but was not provided")]
    AddressNotProvided,
    /// Expected a different function id while decoding function input.
    #[error("expected input id 0x{expected:08x}, got 0x{id:08x}")]
    InputIdMismatch {
        /// Function input id.
        expected: u32,
        /// Id from parsed data.
        id: u32,
    },
    /// Expected a different function id while decoding function output.
    #[error("expected output id 0x{expected:08x}, got 0x{id:08x}")]
    OutputIdMismatch {
        /// Function output id.
        expected: u32,
        /// Id from parsed data.
        id: u32,
    },
    /// Explicitly provided public key cannot be inserted into fields.
    #[error("explicit pubkey was not used")]
    PubkeyNotUsed,
}
