use std::borrow::Cow;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use ahash::HashMap;
use anyhow::Result;
use serde::Deserialize;
use sha2::Digest;

use super::error::AbiError;
use super::{AbiHeaderType, AbiType, AbiValue, AbiVersion, NamedAbiType, NamedAbiValue};
use crate::abi::AbiHeader;
use crate::abi::value::ser::AbiSerializer;
use crate::cell::{
    Cell, CellBuilder, CellDataBuilder, CellFamily, CellSlice, DynCell, HashBytes, Size, Store,
};
use crate::dict::RawDict;
use crate::models::{
    ExtInMsgInfo, IntAddr, MsgInfo, OwnedMessage, OwnedRelaxedMessage, RelaxedIntMsgInfo,
    RelaxedMsgInfo, StateInit, StdAddr,
};
use crate::num::Tokens;
use crate::prelude::Dict;

/// Contract ABI definition.
pub struct Contract {
    /// ABI version.
    pub abi_version: AbiVersion,

    /// List of headers for external messages.
    ///
    /// NOTE: header order matters.
    pub headers: Arc<[AbiHeaderType]>,

    /// A mapping with all contract methods by name.
    pub functions: HashMap<Arc<str>, Function>,

    /// A mapping with all contract events by name.
    pub events: HashMap<Arc<str>, Event>,

    /// Contract init data.
    pub init_data: ContractInitData,

    /// Contract storage fields.
    pub fields: Arc<[NamedAbiType]>,
}

pub enum ContractInitData {
    Dict(HashMap<Arc<str>, (u64, NamedAbiType)>),
    PlainFields(HashSet<Arc<str>>),
}

impl Contract {
    /// Finds a method declaration with the specfied id.
    pub fn find_function_by_id(&self, id: u32, input: bool) -> Option<&Function> {
        self.functions
            .values()
            .find(|item| input && item.input_id == id || !input && item.output_id == id)
    }

    /// Finds an event with the specified id.
    pub fn find_event_by_id(&self, id: u32) -> Option<&Event> {
        self.events.values().find(|event| event.id == id)
    }

    /// Returns a new init data with replaced items.
    ///
    /// NOTE: `tokens` can be a subset of init data fields, all other
    /// will not be touched.
    pub fn update_init_data(
        &self,
        pubkey: Option<&ed25519_dalek::VerifyingKey>,
        tokens: &[NamedAbiValue],
        data: &Cell,
    ) -> Result<Cell> {
        if self.abi_version < AbiVersion::V2_4 {
            self.update_init_data_internal(pubkey, tokens, data)
        } else {
            self.pack_init_fields_into_cell(tokens)
        }
    }

    fn pack_init_fields_into_cell(&self, tokens: &[NamedAbiValue]) -> Result<Cell> {
        let ContractInitData::PlainFields(init_data) = &self.init_data else {
            anyhow::bail!("Dict init_data is not supported for ABI version >= 2.4")
        };

        let mut init_values = Vec::with_capacity(self.fields.len());

        let mut tokens_map = HashMap::<Arc<str>, AbiValue>::with_capacity_and_hasher(
            tokens.len(),
            Default::default(),
        );

        for i in tokens {
            let i = i.clone();
            tokens_map.insert(i.name, i.value);
        }

        for i in self.fields.as_ref() {
            let token = tokens_map.remove_entry(i.name.as_ref());

            if init_data.get(i.name.as_ref()).is_some() {
                let (_, value) = token.ok_or(AbiError::UnexpectedInitDataParam(i.name.clone()))?;
                init_values.push(value)
            } else {
                if let Some((name, _)) = token {
                    return Err(AbiError::UnexpectedInitDataParam(name).into());
                }
                init_values.push(i.ty.make_default_value())
            }
        }
        let cell = AbiValue::tuple_to_cell(init_values.as_ref(), self.abi_version)?;
        Ok(cell)
    }

    fn update_init_data_internal(
        &self,
        pubkey: Option<&ed25519_dalek::VerifyingKey>,
        tokens: &[NamedAbiValue],
        data: &Cell,
    ) -> Result<Cell> {
        // Always check if data is valid
        let mut result = data.parse::<RawDict<64>>()?;

        if pubkey.is_none() && tokens.is_empty() {
            // Do nothing if no items are updated
            return Ok(data.clone());
        }

        let ContractInitData::Dict(init_data) = &self.init_data else {
            anyhow::bail!("Plain init_data is not supported for ABI version < 2.4")
        };

        let context = Cell::empty_context();
        let mut key_builder = CellDataBuilder::new();

        for token in tokens {
            let Some((key, ty)) = init_data.get(token.name.as_ref()) else {
                anyhow::bail!(AbiError::UnexpectedInitDataParam(token.name.clone()));
            };
            token.check_type(ty)?;

            key_builder.store_u64(*key)?;

            result.set_ext(
                key_builder.as_data_slice(),
                &token.make_builder(self.abi_version)?.as_full_slice(),
                context,
            )?;

            key_builder.clear_bits();
        }

        // Set public key if specified
        if let Some(pubkey) = pubkey {
            key_builder.store_u64(0)?;
            result.set_ext(
                key_builder.as_data_slice(),
                &CellBuilder::from_raw_data(pubkey.as_bytes(), 256)?.as_data_slice(),
                context,
            )?;
        }

        // Encode init data as mapping
        CellBuilder::build_from_ext(result, context).map_err(From::from)
    }

    /// Encodes an account data with the specified initial parameters.
    ///
    /// NOTE: `tokens` can be a subset of init data fields, all other
    /// will be set to default.
    pub fn encode_init_data(
        &self,
        pubkey: &ed25519_dalek::VerifyingKey,
        tokens: &[NamedAbiValue],
    ) -> Result<Cell> {
        if self.abi_version < AbiVersion::V2_4 {
            self.encode_init_data_internal(pubkey, tokens)
        } else {
            self.pack_init_fields_into_cell(tokens)
        }
    }

    fn encode_init_data_internal(
        &self,
        pubkey: &ed25519_dalek::VerifyingKey,
        tokens: &[NamedAbiValue],
    ) -> Result<Cell> {
        let mut result = RawDict::<64>::new();

        let ContractInitData::Dict(init_data) = &self.init_data else {
            anyhow::bail!("Plain init fields are not supported")
        };

        let mut init_data = init_data
            .iter()
            .map(|(name, value)| (name.as_ref(), value))
            .collect::<HashMap<_, _>>();

        let context = Cell::empty_context();
        let mut key_builder = CellDataBuilder::new();

        // Write explicitly provided values
        for token in tokens {
            let Some((key, ty)) = init_data.remove(token.name.as_ref()) else {
                anyhow::bail!(AbiError::UnexpectedInitDataParam(token.name.clone()));
            };
            token.check_type(ty)?;

            key_builder.store_u64(*key)?;

            result.set_ext(
                key_builder.as_data_slice(),
                &token.make_builder(self.abi_version)?.as_full_slice(),
                context,
            )?;

            key_builder.clear_bits();
        }

        // Write remaining values with default values
        for (key, ty) in init_data.into_values() {
            key_builder.store_u64(*key)?;

            result.set_ext(
                key_builder.as_data_slice(),
                &ty.make_default_value()
                    .make_builder(self.abi_version)?
                    .as_full_slice(),
                context,
            )?;

            key_builder.clear_bits();
        }

        // Set public key
        key_builder.store_u64(0)?;
        result.set_ext(
            key_builder.as_data_slice(),
            &CellBuilder::from_raw_data(pubkey.as_bytes(), 256)?.as_data_slice(),
            context,
        )?;

        // Encode init data as mapping
        CellBuilder::build_from_ext(result, context).map_err(From::from)
    }

    /// Tries to parse init data fields of this contract from an account data.
    pub fn decode_init_data(&self, data: &DynCell) -> Result<Vec<NamedAbiValue>> {
        if self.abi_version < AbiVersion::V2_4 {
            self.decode_init_data_internal(data)
        } else {
            self.decode_init_fields(data)
        }
    }

    fn decode_init_data_internal(&self, data: &DynCell) -> Result<Vec<NamedAbiValue>> {
        let init_data = data.parse::<Dict<u64, CellSlice>>()?;

        let ContractInitData::Dict(init_data_map) = &self.init_data else {
            anyhow::bail!("Plain init fields are not supported")
        };
        let mut result = Vec::with_capacity(init_data_map.len());

        for (key, item) in init_data_map.values() {
            let Some(mut value) = init_data.get(key)? else {
                anyhow::bail!(AbiError::InitDataFieldNotFound(item.name.clone()));
            };
            result.push(NamedAbiValue::load(item, self.abi_version, &mut value)?);
        }

        Ok(result)
    }

    fn decode_init_fields(&self, data: &DynCell) -> Result<Vec<NamedAbiValue>> {
        let ContractInitData::PlainFields(init_fields) = &self.init_data else {
            anyhow::bail!("Plain init fields are not supported")
        };

        let values = self.decode_fields(data.as_slice()?)?;

        let mut init_values = Vec::with_capacity(init_fields.len());

        for i in values {
            if init_fields.contains(i.name.as_ref()) {
                init_values.push(i)
            }
        }

        Ok(init_values)
    }

    /// Encodes an account data with the specified storage fields of this contract.
    pub fn encode_fields(&self, tokens: &[NamedAbiValue]) -> Result<CellBuilder> {
        ok!(NamedAbiValue::check_types(tokens, &self.fields));
        NamedAbiValue::tuple_to_builder(tokens, self.abi_version).map_err(From::from)
    }

    /// Tries to parse storage fields of this contract from an account data.
    ///
    /// NOTE: The slice is required to contain nothing other than these fields.
    pub fn decode_fields(&self, mut slice: CellSlice<'_>) -> Result<Vec<NamedAbiValue>> {
        self.decode_fields_ext(&mut slice, false)
    }

    /// Tries to parse storage fields of this contract from an account data.
    pub fn decode_fields_ext(
        &self,
        slice: &mut CellSlice<'_>,
        allow_partial: bool,
    ) -> Result<Vec<NamedAbiValue>> {
        let res = ok!(NamedAbiValue::load_tuple_ext(
            &self.fields,
            self.abi_version,
            true,
            allow_partial,
            slice
        ));
        ok!(AbiValue::check_remaining(slice, allow_partial));
        Ok(res)
    }
}

impl<'de> Deserialize<'de> for Contract {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        #[repr(transparent)]
        struct Id(u32);

        impl<'de> Deserialize<'de> for Id {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                #[derive(Deserialize)]
                #[serde(transparent)]
                struct Id<'a>(#[serde(borrow)] Cow<'a, str>);

                match ok!(Id::deserialize(deserializer)).0.strip_prefix("0x") {
                    Some(s) => u32::from_str_radix(s, 16).map(Self).map_err(Error::custom),
                    None => Err(Error::custom("Hex number must be prefixed with 0x")),
                }
            }
        }

        #[derive(Deserialize)]
        struct SerdeNamedAbiType {
            #[serde(flatten)]
            named_abi_type: NamedAbiType,
            #[serde(default)]
            init: bool,
        }

        #[derive(Deserialize)]
        struct SerdeContract {
            #[serde(default, rename = "ABI version")]
            abi_version: Option<u8>,
            #[serde(default)]
            version: Option<String>,
            #[serde(default)]
            header: Vec<AbiHeaderType>,
            functions: Vec<SerdeFunction>,
            #[serde(default)]
            events: Vec<SerdeEvent>,
            #[serde(default)]
            data: Vec<InitData>,
            #[serde(default)]
            fields: Vec<SerdeNamedAbiType>,
        }

        #[derive(Deserialize)]
        struct SerdeFunction {
            name: String,
            #[serde(default)]
            inputs: Vec<NamedAbiType>,
            #[serde(default)]
            outputs: Vec<NamedAbiType>,
            #[serde(default)]
            id: Option<Id>,
        }

        #[derive(Deserialize)]
        struct SerdeEvent {
            name: String,
            #[serde(default)]
            inputs: Vec<NamedAbiType>,
            #[serde(default)]
            id: Option<Id>,
        }

        #[derive(Deserialize)]
        struct InitData {
            key: u64,
            #[serde(flatten)]
            ty: NamedAbiType,
        }

        let contract = ok!(SerdeContract::deserialize(deserializer));
        let abi_version = if let Some(version) = &contract.version {
            ok!(AbiVersion::from_str(version).map_err(Error::custom))
        } else if let Some(major) = contract.abi_version {
            AbiVersion::new(major, 0)
        } else {
            return Err(Error::custom("No ABI version found"));
        };

        let headers = Arc::<[AbiHeaderType]>::from(contract.header);

        let functions = contract
            .functions
            .into_iter()
            .map(|item| {
                let (input_id, output_id) = match item.id {
                    Some(Id(id)) => (id, id),
                    None => {
                        let id = Function::compute_function_id(
                            abi_version,
                            &item.name,
                            headers.as_ref(),
                            &item.inputs,
                            &item.outputs,
                        );
                        (id & Function::INPUT_ID_MASK, id | !Function::INPUT_ID_MASK)
                    }
                };
                let name = Arc::<str>::from(item.name);
                (name.clone(), Function {
                    abi_version,
                    name,
                    headers: headers.clone(),
                    inputs: Arc::from(item.inputs),
                    outputs: Arc::from(item.outputs),
                    input_id,
                    output_id,
                })
            })
            .collect();

        let events = contract
            .events
            .into_iter()
            .map(|item| {
                let id = match item.id {
                    Some(Id(id)) => id,
                    None => {
                        let id = Event::compute_event_id(abi_version, &item.name, &item.inputs);
                        id & Function::INPUT_ID_MASK
                    }
                };
                let name = Arc::<str>::from(item.name);
                (name.clone(), Event {
                    abi_version,
                    name,
                    inputs: Arc::from(item.inputs),
                    id,
                })
            })
            .collect();

        let init_data = if abi_version >= AbiVersion::V2_4 {
            let init_fields = contract
                .fields
                .iter()
                .filter(|x| x.init)
                .map(|x| x.named_abi_type.name.clone())
                .collect();
            ContractInitData::PlainFields(init_fields)
        } else {
            let data = contract
                .data
                .into_iter()
                .map(|item| {
                    let name = item.ty.name.clone();
                    (name, (item.key, item.ty))
                })
                .collect();

            ContractInitData::Dict(data)
        };

        Ok(Self {
            abi_version,
            headers,
            functions,
            events,
            init_data,
            fields: Arc::from(
                contract
                    .fields
                    .into_iter()
                    .map(|x| x.named_abi_type)
                    .collect::<Vec<_>>(),
            ),
        })
    }
}

/// Contract method ABI definition.
#[derive(Debug, Clone)]
pub struct Function {
    /// ABI version (same as contract ABI version).
    pub abi_version: AbiVersion,
    /// List of headers for external messages.
    /// Must be the same as in contract.
    ///
    /// NOTE: header order matters.
    pub headers: Arc<[AbiHeaderType]>,
    /// Method name.
    pub name: Arc<str>,
    /// Method input argument types.
    pub inputs: Arc<[NamedAbiType]>,
    /// Method output types.
    pub outputs: Arc<[NamedAbiType]>,
    /// Function id in incoming messages (with input).
    pub input_id: u32,
    /// Function id in outgoing messages (with output).
    pub output_id: u32,
}

impl Function {
    /// Mask with a bit difference of input id and output id.
    pub const INPUT_ID_MASK: u32 = 0x7fffffff;

    /// Computes function id from the full function signature.
    pub fn compute_function_id(
        abi_version: AbiVersion,
        name: &str,
        headers: &[AbiHeaderType],
        inputs: &[NamedAbiType],
        outputs: &[NamedAbiType],
    ) -> u32 {
        let mut hasher = sha2::Sha256::new();
        FunctionSignatureRaw {
            abi_version,
            name,
            headers,
            inputs,
            outputs,
        }
        .update_hasher(&mut hasher);

        let hash: [u8; 32] = hasher.finalize().into();
        u32::from_be_bytes(hash[0..4].try_into().unwrap())
    }

    /// Encodes message body without headers.
    pub fn encode_internal_msg_body(
        version: AbiVersion,
        id: u32,
        tokens: &[NamedAbiValue],
    ) -> Result<CellBuilder> {
        let mut serializer = AbiSerializer::new(version);
        let output_id = AbiValue::uint(32, id);

        serializer.reserve_value(&output_id);
        for token in tokens {
            serializer.reserve_value(&token.value);
        }

        let context = Cell::empty_context();
        serializer.write_value(&output_id, version, context)?;
        serializer.write_tuple(tokens, version, context)?;
        serializer.finalize(context).map_err(From::from)
    }

    /// Returns a method builder with the specified ABI version and name.
    #[inline]
    pub fn builder<T: Into<String>>(abi_version: AbiVersion, name: T) -> FunctionBuilder {
        FunctionBuilder::new(abi_version, name)
    }

    /// Encodes an unsigned body with invocation of this method as an external message.
    pub fn encode_external<'f, 'a: 'f>(
        &'f self,
        tokens: &'a [NamedAbiValue],
    ) -> ExternalInput<'f, 'a> {
        ExternalInput {
            function: self,
            tokens,
            time: None,
            expire_at: None,
            pubkey: None,
            address: None,
        }
    }

    /// Tries to parse input arguments for this method from an external message body.
    ///
    /// NOTE: The slice is required to contain nothing other than these arguments.
    pub fn decode_external_input(&self, mut slice: CellSlice<'_>) -> Result<Vec<NamedAbiValue>> {
        self.decode_external_input_ext(&mut slice, false)
    }

    /// Tries to parse input arguments for this method from an external message body.
    pub fn decode_external_input_ext(
        &self,
        slice: &mut CellSlice<'_>,
        allow_partial: bool,
    ) -> Result<Vec<NamedAbiValue>> {
        // Read prefix
        let id = if self.abi_version.major == 1 {
            // Load input id
            let id = slice.load_u32()?;
            // Skip signature
            slice.load_reference()?;
            // Skip headers
            ok!(AbiHeader::skip_all(&self.headers, slice));

            id
        } else {
            // Skip signature
            if slice.load_bit()? {
                slice.skip_first(512, 0)?;
            }
            // Skip headers
            ok!(AbiHeader::skip_all(&self.headers, slice));
            // Load input id
            slice.load_u32()?
        };

        // Check input id
        anyhow::ensure!(id == self.input_id, AbiError::InputIdMismatch {
            expected: self.input_id,
            id
        });

        let res = ok!(NamedAbiValue::load_tuple_ext(
            &self.inputs,
            self.abi_version,
            true,
            allow_partial,
            slice
        ));
        ok!(AbiValue::check_remaining(slice, allow_partial));
        Ok(res)
    }

    /// Encodes a message body with invocation of this method as an internal message.
    pub fn encode_internal_input(&self, tokens: &[NamedAbiValue]) -> Result<CellBuilder> {
        ok!(NamedAbiValue::check_types(tokens, &self.inputs));
        Self::encode_internal_msg_body(self.abi_version, self.input_id, tokens)
    }

    /// Encodes an internal message with invocation of this method.
    pub fn encode_internal_message(
        &self,
        tokens: &[NamedAbiValue],
        dst: IntAddr,
        value: Tokens,
        bounce: bool,
        state_init: Option<&StateInit>,
    ) -> Result<Box<OwnedRelaxedMessage>> {
        let body = self.encode_internal_input(tokens)?;
        let body = body.build()?;

        Ok(Box::new(OwnedRelaxedMessage {
            info: RelaxedMsgInfo::Int(RelaxedIntMsgInfo {
                dst,
                bounce,
                value: value.into(),
                ..Default::default()
            }),
            body: body.into(),
            init: state_init.cloned(),
            layout: None,
        }))
    }

    /// Tries to parse input arguments for this method from an internal message body.
    ///
    /// NOTE: The slice is required to contain nothing other than these arguments.
    pub fn decode_internal_input(&self, mut slice: CellSlice<'_>) -> Result<Vec<NamedAbiValue>> {
        self.decode_internal_input_ext(&mut slice, false)
    }

    /// Tries to parse input arguments for this method from an internal message body.
    pub fn decode_internal_input_ext(
        &self,
        slice: &mut CellSlice<'_>,
        allow_partial: bool,
    ) -> Result<Vec<NamedAbiValue>> {
        let id = slice.load_u32()?;
        anyhow::ensure!(id == self.input_id, AbiError::InputIdMismatch {
            expected: self.input_id,
            id
        });
        let res = ok!(NamedAbiValue::load_tuple_ext(
            &self.inputs,
            self.abi_version,
            true,
            allow_partial,
            slice
        ));
        ok!(AbiValue::check_remaining(slice, allow_partial));
        Ok(res)
    }

    /// Encodes a message body with result of invocation of this method.
    pub fn encode_output(&self, tokens: &[NamedAbiValue]) -> Result<CellBuilder> {
        ok!(NamedAbiValue::check_types(tokens, &self.outputs));
        Self::encode_internal_msg_body(self.abi_version, self.output_id, tokens)
    }

    /// Tries to parse output arguments of this method from a message body.
    ///
    /// NOTE: The slice is required to contain nothing other than these arguments.
    pub fn decode_output(&self, mut slice: CellSlice<'_>) -> Result<Vec<NamedAbiValue>> {
        self.decode_output_ext(&mut slice, false)
    }

    /// Tries to parse output arguments of this method from a message body.
    pub fn decode_output_ext(
        &self,
        slice: &mut CellSlice<'_>,
        allow_partial: bool,
    ) -> Result<Vec<NamedAbiValue>> {
        let id = slice.load_u32()?;
        anyhow::ensure!(id == self.output_id, AbiError::OutputIdMismatch {
            expected: self.output_id,
            id
        });
        let res = ok!(NamedAbiValue::load_tuple_ext(
            &self.outputs,
            self.abi_version,
            true,
            allow_partial,
            slice
        ));
        ok!(AbiValue::check_remaining(slice, allow_partial));
        Ok(res)
    }

    /// Returns an object which can be used to display the normalized signature of this method.
    pub fn display_signature(&self) -> impl std::fmt::Display + '_ {
        FunctionSignatureRaw {
            abi_version: self.abi_version,
            name: &self.name,
            headers: &self.headers,
            inputs: &self.inputs,
            outputs: &self.outputs,
        }
    }
}

/// External input builder.
#[derive(Clone)]
pub struct ExternalInput<'f, 'a> {
    function: &'f Function,
    tokens: &'a [NamedAbiValue],
    time: Option<u64>,
    expire_at: Option<u32>,
    pubkey: Option<&'a ed25519_dalek::VerifyingKey>,
    address: Option<&'a StdAddr>,
}

impl<'a> ExternalInput<'_, 'a> {
    /// Builds an external message to the specified address.
    pub fn build_message(&self, address: &StdAddr) -> Result<UnsignedExternalMessage> {
        Ok(ok!(self.build_input_ext(Some(address))).with_dst(address.clone()))
    }

    /// Builds an exteranl message to the specified address without signature.
    ///
    /// Returns an expiration timestamp along with message.
    pub fn build_message_without_signature(
        &self,
        address: &StdAddr,
    ) -> Result<(u32, OwnedMessage)> {
        let (expire_at, body) = ok!(self.build_input_without_signature());
        Ok((expire_at, OwnedMessage {
            info: MsgInfo::ExtIn(ExtInMsgInfo {
                dst: IntAddr::Std(address.clone()),
                ..Default::default()
            }),
            body: body.into(),
            init: None,
            layout: None,
        }))
    }

    /// Builds an external message body.
    pub fn build_input(&self) -> Result<UnsignedBody> {
        self.build_input_ext(self.address)
    }

    fn build_input_ext(&self, address: Option<&StdAddr>) -> Result<UnsignedBody> {
        let (expire_at, payload) = self.build_payload(true)?;

        let context = Cell::empty_context();
        let hash = if self.function.abi_version >= AbiVersion::V2_3 {
            let mut to_sign = CellBuilder::new();
            match address {
                Some(address) => address.store_into(&mut to_sign, context)?,
                None => anyhow::bail!(AbiError::AddressNotProvided),
            };
            to_sign.store_slice(payload.as_slice()?)?;
            *to_sign.build_ext(context)?.repr_hash()
        } else {
            *payload.repr_hash()
        };

        Ok(UnsignedBody {
            abi_version: self.function.abi_version,
            expire_at,
            payload,
            hash,
        })
    }

    /// Builds an external message body without signature.
    pub fn build_input_without_signature(&self) -> Result<(u32, Cell)> {
        self.build_payload(false)
    }

    fn build_payload(&self, reserve_signature: bool) -> Result<(u32, Cell)> {
        const DEFAULT_TIMEOUT_SEC: u32 = 60;

        fn now_ms() -> u64 {
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
        }

        ok!(NamedAbiValue::check_types(
            self.tokens,
            &self.function.inputs
        ));

        let abi_version = self.function.abi_version;

        let mut serializer = AbiSerializer::new(abi_version);

        serializer.add_offset(if abi_version.major == 1 {
            // Reserve reference for signature
            Size { bits: 0, refs: 1 }
        } else {
            let bits = if abi_version >= AbiVersion::V2_3 {
                // Reserve only for address as it also ensures the the signature will fit
                IntAddr::BITS_MAX
            } else {
                // Reserve for `Some` non-empty signature
                1 + 512
            };
            Size { bits, refs: 0 }
        });

        let input_id = AbiValue::uint(32, self.function.input_id);

        serializer.reserve_headers(&self.function.headers, self.pubkey.is_some());
        serializer.reserve_value(&input_id);
        for token in self.tokens {
            serializer.reserve_value(&token.value);
        }

        let context = Cell::empty_context();

        if !reserve_signature {
            let value = if abi_version.major == 1 {
                AbiValue::Cell(Cell::default())
            } else {
                AbiValue::Bool(false)
            };
            serializer.reserve_value(&value);
            serializer.write_value(&value, abi_version, context)?;
        }

        let time = self.time.unwrap_or_else(now_ms);
        let expire_at = self
            .expire_at
            .unwrap_or((time / 1000) as u32 + DEFAULT_TIMEOUT_SEC);

        for header in self.function.headers.as_ref() {
            serializer.write_header_value(&match header {
                AbiHeaderType::Time => AbiHeader::Time(time),
                AbiHeaderType::Expire => AbiHeader::Expire(expire_at),
                AbiHeaderType::PublicKey => {
                    AbiHeader::PublicKey(self.pubkey.map(|key| Box::new(*key)))
                }
            })?;
        }

        serializer.write_value(&input_id, abi_version, context)?;
        serializer.write_tuple(self.tokens, abi_version, context)?;

        let payload = serializer.finalize(context)?.build_ext(context)?;
        Ok((expire_at, payload))
    }

    /// Use the specified time for the `time` header.
    #[inline]
    pub fn set_time(&mut self, time: u64) {
        self.time = Some(time);
    }

    /// Use the specified time for the `time` header.
    #[inline]
    pub fn with_time(mut self, time: u64) -> Self {
        self.set_time(time);
        self
    }

    /// Use the specified expiration timestamp for the `expire` header.
    #[inline]
    pub fn set_expire_at(&mut self, expire_at: u32) {
        self.expire_at = Some(expire_at);
    }

    /// Use the specified expiration timestamp for the `expire` header.
    #[inline]
    pub fn with_expire_at(mut self, expire_at: u32) -> Self {
        self.set_expire_at(expire_at);
        self
    }

    /// Use the specified public key for the `pubkey` header.
    #[inline]
    pub fn set_pubkey(&mut self, pubkey: &'a ed25519_dalek::VerifyingKey) {
        self.pubkey = Some(pubkey);
    }

    /// Use the specified public key for the `pubkey` header.
    #[inline]
    pub fn with_pubkey(mut self, pubkey: &'a ed25519_dalek::VerifyingKey) -> Self {
        self.set_pubkey(pubkey);
        self
    }

    /// Use the specified address for ABIv2.3 signature.
    #[inline]
    pub fn set_address(&mut self, address: &'a StdAddr) {
        self.address = Some(address);
    }

    /// Use the specified address for ABIv2.3 signature.
    #[inline]
    pub fn with_address(mut self, address: &'a StdAddr) -> Self {
        self.set_address(address);
        self
    }
}

/// Method ABI declaration builder.
#[derive(Debug, Clone)]
pub struct FunctionBuilder {
    abi_version: AbiVersion,
    name: String,
    headers: Vec<AbiHeaderType>,
    inputs: Vec<NamedAbiType>,
    outputs: Vec<NamedAbiType>,
    id: Option<u32>,
}

impl FunctionBuilder {
    /// Creates an empty ABI declaration for a method with the specified ABI version and name.
    pub fn new<T: Into<String>>(abi_version: AbiVersion, name: T) -> Self {
        Self {
            abi_version,
            name: name.into(),
            headers: Default::default(),
            inputs: Default::default(),
            outputs: Default::default(),
            id: None,
        }
    }

    /// Finalizes an ABI declaration.
    pub fn build(self) -> Function {
        let (input_id, output_id) = match self.id {
            Some(id) => (id, id),
            None => {
                let id = Function::compute_function_id(
                    self.abi_version,
                    &self.name,
                    &self.headers,
                    &self.inputs,
                    &self.outputs,
                );
                (id & Function::INPUT_ID_MASK, id | !Function::INPUT_ID_MASK)
            }
        };

        Function {
            abi_version: self.abi_version,
            headers: Arc::from(self.headers),
            name: Arc::from(self.name),
            inputs: Arc::from(self.inputs),
            outputs: Arc::from(self.outputs),
            input_id,
            output_id,
        }
    }

    /// Sets method headers to the specified list.
    pub fn with_headers<I: IntoIterator<Item = AbiHeaderType>>(mut self, headers: I) -> Self {
        self.headers = headers.into_iter().collect();
        self
    }

    /// Sets method input types to the specified list of named arguments.
    pub fn with_inputs<I, T>(mut self, inputs: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<NamedAbiType>,
    {
        self.inputs = inputs.into_iter().map(Into::into).collect();
        self
    }

    /// Sets method input types to the specified list of unnamed arguments.
    pub fn with_unnamed_inputs<I: IntoIterator<Item = AbiType>>(mut self, inputs: I) -> Self {
        self.inputs = inputs
            .into_iter()
            .enumerate()
            .map(NamedAbiType::from)
            .collect();
        self
    }

    /// Sets method output types to the specified list of named arguments.
    pub fn with_outputs<I, T>(mut self, outputs: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<NamedAbiType>,
    {
        self.outputs = outputs.into_iter().map(Into::into).collect();
        self
    }

    /// Sets method output types to the specified list of unnamed arguments.
    pub fn with_unnamed_outputs<I: IntoIterator<Item = AbiType>>(mut self, outputs: I) -> Self {
        self.outputs = outputs
            .into_iter()
            .enumerate()
            .map(NamedAbiType::from)
            .collect();
        self
    }

    /// Sets an explicit function id.
    pub fn with_id(mut self, id: u32) -> Self {
        self.id = Some(id);
        self
    }
}

/// Contract event ABI definition.
#[derive(Debug, Clone)]
pub struct Event {
    /// ABI version (same as contract ABI version).
    pub abi_version: AbiVersion,
    /// Event name.
    pub name: Arc<str>,
    /// Event arguments.
    pub inputs: Arc<[NamedAbiType]>,
    /// Event id derived from signature.
    pub id: u32,
}

impl Event {
    /// Computes event id from the full event signature.
    pub fn compute_event_id(abi_version: AbiVersion, name: &str, inputs: &[NamedAbiType]) -> u32 {
        let mut hasher = sha2::Sha256::new();
        EventSignatureRaw {
            abi_version,
            name,
            inputs,
        }
        .update_hasher(&mut hasher);

        let hash: [u8; 32] = hasher.finalize().into();
        u32::from_be_bytes(hash[0..4].try_into().unwrap())
    }

    /// Returns an event builder with the specified ABI version and name.
    #[inline]
    pub fn builder<T: Into<String>>(abi_version: AbiVersion, name: T) -> EventBuilder {
        EventBuilder::new(abi_version, name)
    }

    /// Encodes a message body with this event as an internal message.
    pub fn encode_internal_input(&self, tokens: &[NamedAbiValue]) -> Result<CellBuilder> {
        ok!(NamedAbiValue::check_types(tokens, &self.inputs));
        Function::encode_internal_msg_body(self.abi_version, self.id, tokens)
    }

    /// Tries to parse input arguments for this event from an internal message body.
    ///
    /// NOTE: The slice is required to contain nothing other than these arguments.
    pub fn decode_internal_input(&self, mut slice: CellSlice<'_>) -> Result<Vec<NamedAbiValue>> {
        self.decode_internal_input_ext(&mut slice, false)
    }

    /// Tries to parse input arguments for this event from an internal message body.
    pub fn decode_internal_input_ext(
        &self,
        slice: &mut CellSlice<'_>,
        allow_partial: bool,
    ) -> Result<Vec<NamedAbiValue>> {
        let id = slice.load_u32()?;
        anyhow::ensure!(id == self.id, AbiError::InputIdMismatch {
            expected: self.id,
            id
        });
        let res = ok!(NamedAbiValue::load_tuple_ext(
            &self.inputs,
            self.abi_version,
            true,
            allow_partial,
            slice
        ));
        ok!(AbiValue::check_remaining(slice, allow_partial));
        Ok(res)
    }

    /// Returns an object which can be used to display the normalized signature of this event.
    pub fn display_signature(&self) -> impl std::fmt::Display + '_ {
        EventSignatureRaw {
            abi_version: self.abi_version,
            name: &self.name,
            inputs: &self.inputs,
        }
    }
}

/// Event ABI declaration builder.
#[derive(Debug, Clone)]
pub struct EventBuilder {
    abi_version: AbiVersion,
    name: String,
    inputs: Vec<NamedAbiType>,
    id: Option<u32>,
}

impl EventBuilder {
    /// Creates an empty ABI declaration for an event with the specified ABI version and name.
    pub fn new<T: Into<String>>(abi_version: AbiVersion, name: T) -> Self {
        Self {
            abi_version,
            name: name.into(),
            inputs: Default::default(),
            id: None,
        }
    }

    /// Finalizes an ABI declaration.
    pub fn build(self) -> Event {
        let id = match self.id {
            Some(id) => id,
            None => {
                let id = Event::compute_event_id(self.abi_version, &self.name, &self.inputs);
                id & Function::INPUT_ID_MASK
            }
        };

        Event {
            abi_version: self.abi_version,
            name: Arc::from(self.name),
            inputs: Arc::from(self.inputs),
            id,
        }
    }

    /// Sets event input types to the specified list of named arguments.
    pub fn with_inputs<I, T>(mut self, inputs: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<NamedAbiType>,
    {
        self.inputs = inputs.into_iter().map(Into::into).collect();
        self
    }

    /// Sets event input types to the specified list of unnamed arguments.
    pub fn with_unnamed_inputs<I: IntoIterator<Item = AbiType>>(mut self, inputs: I) -> Self {
        self.inputs = inputs
            .into_iter()
            .enumerate()
            .map(NamedAbiType::from)
            .collect();
        self
    }

    /// Sets an explicit event id.
    pub fn with_id(mut self, id: u32) -> Self {
        self.id = Some(id);
        self
    }
}

/// Unsigned external message.
pub struct UnsignedExternalMessage {
    /// Destination contract address.
    pub dst: StdAddr,
    /// Unsigned payload.
    pub body: UnsignedBody,
    /// Optional initial contract state.
    pub init: Option<StateInit>,
}

impl UnsignedExternalMessage {
    /// Updates the state init of the external message to the specified one.
    pub fn set_state_init(&mut self, init: Option<StateInit>) {
        self.init = init;
    }

    /// Returns an external message with the specified state init.
    pub fn with_state_init(mut self, init: StateInit) -> Self {
        self.set_state_init(Some(init));
        self
    }

    /// Returns the expiration timestamp of this message.
    #[inline]
    pub fn expire_at(&self) -> u32 {
        self.body.expire_at
    }

    /// Signs the payload and returns an external message with filled signature.
    pub fn sign(
        self,
        key: &ed25519_dalek::SigningKey,
        signature_id: Option<i32>,
    ) -> Result<OwnedMessage> {
        let signature =
            crate::abi::sign_with_signature_id(key, self.body.hash.as_slice(), signature_id);
        self.with_signature(&signature)
    }

    /// Returns an external message with filled signature.
    pub fn with_signature(self, signature: &ed25519_dalek::Signature) -> Result<OwnedMessage> {
        self.into_signed(Some(&signature.to_bytes()))
    }

    /// Returns an external message with signature filled with zero bytes.
    pub fn with_fake_signature(self) -> Result<OwnedMessage> {
        self.into_signed(Some(&[0u8; 64]))
    }

    /// Returns an external message with empty signature.
    pub fn without_signature(self) -> Result<OwnedMessage> {
        self.into_signed(None)
    }

    /// Returns an external message with filled signature.
    pub fn fill_signature(&self, signature: Option<&[u8; 64]>) -> Result<OwnedMessage> {
        let body = self.body.fill_signature(signature)?;
        Ok(OwnedMessage {
            info: MsgInfo::ExtIn(ExtInMsgInfo {
                dst: IntAddr::Std(self.dst.clone()),
                ..Default::default()
            }),
            body: body.into(),
            init: self.init.clone(),
            layout: None,
        })
    }

    /// Converts an unsigned message into an external message with filled signature.
    fn into_signed(self, signature: Option<&[u8; 64]>) -> Result<OwnedMessage> {
        let body = self.body.fill_signature(signature)?;
        Ok(OwnedMessage {
            info: MsgInfo::ExtIn(ExtInMsgInfo {
                dst: IntAddr::Std(self.dst),
                ..Default::default()
            }),
            body: body.into(),
            init: self.init,
            layout: None,
        })
    }
}

/// Unsigned external message payload.
pub struct UnsignedBody {
    /// ABI version used during encoding.
    pub abi_version: AbiVersion,
    /// Unsigned payload.
    pub payload: Cell,
    /// A hash to sign.
    pub hash: HashBytes,
    /// Message expiration timestamp (in seconds).
    pub expire_at: u32,
}

impl UnsignedBody {
    /// Extends message with the specified destination address and returns an
    /// unsigned external message.
    pub fn with_dst(self, dst: StdAddr) -> UnsignedExternalMessage {
        UnsignedExternalMessage {
            dst,
            body: self,
            init: None,
        }
    }

    /// Signs the payload and returns a body cell with filled signature.
    pub fn sign(self, key: &ed25519_dalek::SigningKey, signature_id: Option<i32>) -> Result<Cell> {
        let signature = crate::abi::sign_with_signature_id(key, self.hash.as_slice(), signature_id);
        self.with_signature(&signature)
    }

    /// Returns a body cell with filled signature.
    pub fn with_signature(self, signature: &ed25519_dalek::Signature) -> Result<Cell> {
        self.fill_signature(Some(&signature.to_bytes()))
    }

    /// Returns a body cell with signature filled with zero bytes.
    pub fn with_fake_signature(self) -> Result<Cell> {
        self.fill_signature(Some(&[0u8; 64]))
    }

    /// Returns a body cell with empty signature.
    pub fn without_signature(self) -> Result<Cell> {
        self.fill_signature(None)
    }

    /// Returns a body cell with filled signature.
    pub fn fill_signature(&self, signature: Option<&[u8; 64]>) -> Result<Cell> {
        let mut builder = CellBuilder::new();

        if self.abi_version.major == 1 {
            builder.store_reference(match signature {
                Some(signature) => {
                    // TODO: extend with public key?
                    CellBuilder::from_raw_data(signature, 512).and_then(CellBuilder::build)?
                }
                None => Cell::empty_cell(),
            })?;
        } else {
            match signature {
                Some(signature) => {
                    builder.store_bit_one()?;
                    builder.store_raw(signature, 512)?;
                }
                None => builder.store_bit_zero()?,
            }
            builder.store_slice(self.payload.as_slice()?)?;
        }

        builder.build().map_err(From::from)
    }
}

struct FunctionSignatureRaw<'a> {
    abi_version: AbiVersion,
    name: &'a str,
    headers: &'a [AbiHeaderType],
    inputs: &'a [NamedAbiType],
    outputs: &'a [NamedAbiType],
}

impl FunctionSignatureRaw<'_> {
    fn update_hasher<H: Digest>(&self, engine: &mut H) {
        std::fmt::write(&mut DisplayHasher(engine), format_args!("{self}")).unwrap();
    }
}

impl std::fmt::Display for FunctionSignatureRaw<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        ok!(write!(f, "{}(", self.name));

        let mut first = true;
        if self.abi_version.major == 1 {
            for header in self.headers {
                if !std::mem::take(&mut first) {
                    ok!(f.write_str(","));
                }
                ok!(std::fmt::Display::fmt(header, f));
            }
        }
        for item in self.inputs {
            if !std::mem::take(&mut first) {
                ok!(f.write_str(","));
            }
            ok!(std::fmt::Display::fmt(&item.ty, f));
        }

        ok!(f.write_str(")("));

        first = true;
        for item in self.outputs {
            if !std::mem::take(&mut first) {
                ok!(f.write_str(","));
            }
            ok!(std::fmt::Display::fmt(&item.ty, f));
        }

        write!(f, ")v{}", self.abi_version.major)
    }
}

struct EventSignatureRaw<'a> {
    abi_version: AbiVersion,
    name: &'a str,
    inputs: &'a [NamedAbiType],
}

impl EventSignatureRaw<'_> {
    fn update_hasher<H: Digest>(&self, engine: &mut H) {
        std::fmt::write(&mut DisplayHasher(engine), format_args!("{self}")).unwrap();
    }
}

impl std::fmt::Display for EventSignatureRaw<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        ok!(write!(f, "{}(", self.name));

        let mut first = true;
        for item in self.inputs {
            if !std::mem::take(&mut first) {
                ok!(f.write_str(","));
            }
            ok!(std::fmt::Display::fmt(&item.ty, f));
        }
        write!(f, ")v{}", self.abi_version.major)
    }
}

#[repr(transparent)]
struct DisplayHasher<'a, H>(&'a mut H);

impl<H: Digest> std::fmt::Write for DisplayHasher<'_, H> {
    #[inline]
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.update(s.as_bytes());
        Ok(())
    }
}
