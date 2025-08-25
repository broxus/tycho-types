use std::sync::Arc;

use bytes::Bytes;
use num_bigint::BigInt;

use crate::abi::contract::ContractInitData;
use crate::abi::*;
use crate::models::StdAddr;
use crate::prelude::{Cell, CellBuilder, CellFamily, HashBytes, RawDict, Store};

const DEPOOL_ABI: &str = include_str!("depool.abi.json");
const ABI_V2_4: &str = include_str!("contract_v24.abi.json");

#[test]
fn decode_json_abi() {
    let contract = serde_json::from_str::<Contract>(DEPOOL_ABI).unwrap();
    assert_eq!(contract.abi_version, AbiVersion::V2_0);
    assert_eq!(contract.functions.len(), 28);
    assert_eq!(contract.events.len(), 10);
    assert_eq!(contract.fields.len(), 0);
    let ContractInitData::Dict(init_data) = &contract.init_data else {
        panic!("init fields are not supported");
    };
    assert_eq!(init_data.len(), 0);

    let function = contract.find_function_by_id(0x4e73744b, true).unwrap();
    assert_eq!(function.input_id, 0x4e73744b);
    assert_eq!(function.name.as_ref(), "participateInElections");
}

#[test]
fn decode_json_abi_v24() {
    let contract = serde_json::from_str::<Contract>(ABI_V2_4).unwrap();
    assert_eq!(contract.abi_version, AbiVersion::V2_4);
    assert_eq!(contract.functions.len(), 5);
    assert_eq!(contract.events.len(), 3);
    assert_eq!(contract.fields.len(), 2);

    let ContractInitData::PlainFields(fields) = &contract.init_data else {
        panic!("init data is not supported");
    };

    assert_eq!(fields.len(), 1);

    let key = ed25519_dalek::SigningKey::from([0u8; 32]);
    let pubkey = ed25519_dalek::VerifyingKey::from(&key);

    if let Err(e) = contract.encode_init_data(&pubkey, &[NamedAbiValue {
        name: Arc::from("a"),
        value: AbiValue::Int(128, BigInt::from(0)),
    }]) {
        println!("Expected to fail because of unexpected field name. Err: {e:?}");
    }

    let init_values = vec![NamedAbiValue {
        name: Arc::from("b"),
        value: AbiValue::Int(128, BigInt::from(0)),
    }];

    let cell = contract
        .encode_init_data(&pubkey, init_values.as_ref())
        .unwrap();

    let fields = contract.decode_init_data(cell.as_ref()).unwrap();

    assert_eq!(init_values, fields);

    let function = contract.find_function_by_id(0x01234567, true).unwrap();
    assert_eq!(function.input_id, 0x01234567);
    assert_eq!(function.name.as_ref(), "has_id");
}

#[test]
fn test_fixed_bytes() {
    let mut builder = CellBuilder::new();

    let bytes = vec![0u8; 32];
    let value = AbiValue::FixedBytes(Bytes::from(bytes.clone()));
    let cell_2 = value.make_cell(AbiVersion::V2_3).unwrap();
    let cell_24 = value.make_cell(AbiVersion::V2_4).unwrap();
    let bytes_builder = CellBuilder::from_raw_data(bytes.as_ref(), 256).unwrap();

    let mut builder_v24 = builder.clone();
    builder_v24.store_builder(&bytes_builder).unwrap();
    builder
        .store_reference(bytes_builder.build().unwrap())
        .unwrap();

    let built_24 = builder_v24.build().unwrap();
    let built_2 = builder.build().unwrap();

    assert_eq!(built_24, cell_24);
    assert_eq!(built_2, cell_2);

    let value_2 = AbiValue::load(
        &AbiType::FixedBytes(32),
        AbiVersion::V2_3,
        &mut built_2.as_slice().unwrap(),
    )
    .unwrap();
    assert_eq!(value_2, value);

    let value_24 = AbiValue::load(
        &AbiType::FixedBytes(32),
        AbiVersion::V2_4,
        &mut built_24.as_slice().unwrap(),
    )
    .unwrap();
    assert_eq!(value_24, value);
}

#[test]
fn encode_internal_input() {
    let contract = serde_json::from_str::<Contract>(DEPOOL_ABI).unwrap();
    let function = contract.find_function_by_id(0x4e73744b, true).unwrap();

    let expected = {
        let mut builder = CellBuilder::new();
        builder.store_u32(function.input_id).unwrap();
        builder.store_u64(123).unwrap();
        builder.store_zeros(256).unwrap();
        builder.store_u32(321).unwrap();
        builder.store_u32(16123).unwrap();
        builder.store_zeros(256).unwrap();
        builder
            .store_reference({
                let builder = CellBuilder::from_raw_data(&[0; 64], 512).unwrap();
                builder.build().unwrap()
            })
            .unwrap();
        builder.build().unwrap()
    };

    let body = function
        .encode_internal_input(&[
            123u64.into_abi().named("queryId"),
            HashBytes::default().into_abi().named("validatorKey"),
            321u32.into_abi().named("stakeAt"),
            16123u32.into_abi().named("maxFactor"),
            HashBytes::default().into_abi().named("adnlAddr"),
            Bytes::from(vec![0; 64]).into_abi().named("signature"),
        ])
        .unwrap()
        .build()
        .unwrap();

    assert_eq!(body, expected);
}

#[test]
fn decode_internal_input() {
    let contract = serde_json::from_str::<Contract>(DEPOOL_ABI).unwrap();
    let function = contract.find_function_by_id(0x4e73744b, true).unwrap();

    let body = {
        let mut builder = CellBuilder::new();
        builder.store_u32(function.input_id).unwrap();
        builder.store_u64(123).unwrap();
        builder.store_zeros(256).unwrap();
        builder.store_u32(321).unwrap();
        builder.store_u32(16123).unwrap();
        builder.store_zeros(256).unwrap();
        builder
            .store_reference({
                let builder = CellBuilder::from_raw_data(&[0; 64], 512).unwrap();
                builder.build().unwrap()
            })
            .unwrap();
        builder.build().unwrap()
    };

    let tokens = function
        .decode_internal_input(body.as_slice().unwrap())
        .unwrap();

    NamedAbiValue::check_types(&tokens, &function.inputs).unwrap();
}

#[test]
fn encode_external_input() {
    let contract = serde_json::from_str::<Contract>(DEPOOL_ABI).unwrap();
    let function = contract.functions.get("constructor").unwrap();

    let expected = {
        let mut builder = CellBuilder::new();
        builder.store_bit_one().unwrap();
        builder.store_zeros(512).unwrap();
        builder.store_u64(10000).unwrap();
        builder.store_u32(10).unwrap();
        builder.store_u32(function.input_id).unwrap();
        builder.store_u64(123).unwrap();
        builder.store_u64(321).unwrap();
        builder.store_reference(Cell::default()).unwrap();
        builder
            .store_reference({
                let mut builder = CellBuilder::new();
                StdAddr::default()
                    .store_into(&mut builder, Cell::empty_context())
                    .unwrap();
                builder.store_u8(1).unwrap();
                builder.build().unwrap()
            })
            .unwrap();
        builder.build().unwrap()
    };

    let body = function
        .encode_external(&[
            123u64.into_abi().named("minStake"),
            321u64.into_abi().named("validatorAssurance"),
            Cell::default().into_abi().named("proxyCode"),
            StdAddr::default().into_abi().named("validatorWallet"),
            1u8.into_abi().named("participantRewardFraction"),
        ])
        .with_time(10000)
        .with_expire_at(10)
        .build_input()
        .unwrap()
        .with_fake_signature()
        .unwrap();

    assert_eq!(body, expected);
}

#[test]
fn decode_external_input() {
    let contract = serde_json::from_str::<Contract>(DEPOOL_ABI).unwrap();
    println!("{:?}", contract.functions);
    let function = contract.functions.get("constructor").unwrap();

    let body = {
        let mut builder = CellBuilder::new();
        builder.store_bit_one().unwrap();
        builder.store_zeros(512).unwrap();
        builder.store_u64(10000).unwrap();
        builder.store_u32(10).unwrap();
        builder.store_u32(function.input_id).unwrap();
        builder.store_u64(123).unwrap();
        builder.store_u64(321).unwrap();
        builder.store_reference(Cell::default()).unwrap();
        builder
            .store_reference({
                let mut builder = CellBuilder::new();
                StdAddr::default()
                    .store_into(&mut builder, Cell::empty_context())
                    .unwrap();
                builder.store_u8(1).unwrap();
                builder.build().unwrap()
            })
            .unwrap();
        builder.build().unwrap()
    };

    let tokens = function
        .decode_external_input(body.as_slice().unwrap())
        .unwrap();

    NamedAbiValue::check_types(&tokens, &function.inputs).unwrap();
}

#[test]
fn encode_unsigned_external_input() {
    let contract = serde_json::from_str::<Contract>(DEPOOL_ABI).unwrap();
    let function = contract.functions.get("constructor").unwrap();

    let expected = {
        let mut builder = CellBuilder::new();
        builder.store_bit_zero().unwrap();
        builder.store_u64(10000).unwrap();
        builder.store_u32(10).unwrap();
        builder.store_u32(function.input_id).unwrap();
        builder.store_u64(123).unwrap();
        builder.store_u64(321).unwrap();
        builder.store_reference(Cell::default()).unwrap();
        builder
            .store_reference({
                let mut b = CellBuilder::new();
                b.store_slice(
                    CellBuilder::build_from(StdAddr::default())
                        .unwrap()
                        .as_slice()
                        .unwrap(),
                )
                .unwrap();
                b.store_u8(1).unwrap();
                b.build().unwrap()
            })
            .unwrap();
        builder.build().unwrap()
    };

    let (_, body) = function
        .encode_external(&[
            123u64.into_abi().named("minStake"),
            321u64.into_abi().named("validatorAssurance"),
            Cell::default().into_abi().named("proxyCode"),
            StdAddr::default().into_abi().named("validatorWallet"),
            1u8.into_abi().named("participantRewardFraction"),
        ])
        .with_time(10000)
        .with_expire_at(10)
        .build_input_without_signature()
        .unwrap();

    println!("{}", expected.display_tree());
    println!("{}", body.display_tree());

    assert_eq!(body, expected);
}

#[test]
fn decode_unsigned_external_input() {
    let contract = serde_json::from_str::<Contract>(DEPOOL_ABI).unwrap();
    let function = contract.functions.get("constructor").unwrap();

    let body = {
        let mut builder = CellBuilder::new();
        builder.store_bit_zero().unwrap();
        builder.store_u64(10000).unwrap();
        builder.store_u32(10).unwrap();
        builder.store_u32(function.input_id).unwrap();
        builder.store_u64(123).unwrap();
        builder.store_u64(321).unwrap();
        builder.store_reference(Cell::default()).unwrap();
        StdAddr::default()
            .store_into(&mut builder, Cell::empty_context())
            .unwrap();
        builder.store_u8(1).unwrap();
        builder.build().unwrap()
    };

    let tokens = function
        .decode_external_input(body.as_slice().unwrap())
        .unwrap();

    NamedAbiValue::check_types(&tokens, &function.inputs).unwrap();
}

#[test]
fn encode_empty_init_data() {
    let contract = serde_json::from_str::<Contract>(DEPOOL_ABI).unwrap();

    let key = ed25519_dalek::SigningKey::from([0u8; 32]);
    let pubkey = ed25519_dalek::VerifyingKey::from(&key);

    let expected = {
        let mut dict = RawDict::<64>::new();

        let mut key = CellBuilder::new();
        key.store_u64(0).unwrap();

        let value = CellBuilder::from_raw_data(pubkey.as_bytes(), 256).unwrap();
        dict.set(key.as_data_slice(), value.as_data_slice())
            .unwrap();

        CellBuilder::build_from(dict).unwrap()
    };

    let init_data = contract.encode_init_data(&pubkey, &[]).unwrap();

    assert_eq!(init_data, expected);
}

#[test]
fn test_unnamed_derivation() {
    let smt = SomeType("hello".to_string());
    let abi = smt.as_abi();
    let moved_abi = smt.clone().into_abi();

    let abi_value_manual =
        AbiValue::Tuple(vec![AbiValue::String("hello".to_string()).named("value0")]);

    assert_eq!(abi_value_manual, abi);
    assert_eq!(moved_abi, abi);

    let abi_type = SomeType::<String>::abi_type();
    let abi_type_manual = AbiType::Tuple(Arc::from([AbiType::String.named("value0")]));
    assert_eq!(abi_type_manual, abi_type);

    let from_abi = SomeType::<String>::from_abi(abi).unwrap();
    let from_moved_abi = SomeType::<String>::from_abi(moved_abi).unwrap();

    assert_eq!(from_abi, from_moved_abi);
    assert_eq!(from_abi, smt);
}

#[test]
fn test_abi_derivation() {
    let test_value = Test::<Inner> {
        first: "aaa".to_string(),
        second: Inner {
            inner_first: ("bbb".to_string(), "ccc".to_string()),
        },
        third: vec![1, 2, 3],
    };

    let abi = test_value.as_abi();
    let moved_abi = test_value.clone().into_abi();

    let abi_value_manual = AbiValue::Tuple(vec![
        AbiValue::String(String::from("aaa")).named("kek"),
        AbiValue::Tuple(vec![
            AbiValue::Tuple(vec![
                AbiValue::String(String::from("bbb")).named("value0"),
                AbiValue::String(String::from("ccc")).named("value1"),
            ])
            .named("inner_first"),
        ])
        .named("lol"),
        AbiValue::Array(Arc::from(AbiType::Uint(64)), vec![
            AbiValue::uint(64, 1u64),
            AbiValue::uint(64, 2u64),
            AbiValue::uint(64, 3u64),
        ])
        .named("third"),
    ]);

    assert_eq!(abi, moved_abi);
    assert_eq!(abi, abi_value_manual);

    let abi_type = Test::<Inner>::abi_type();
    let abi_type_manual = AbiType::Tuple(std::sync::Arc::from([
        AbiType::String.named("kek"),
        AbiType::Tuple(Arc::from([AbiType::Tuple(Arc::from([
            AbiType::String.named("value0"),
            AbiType::String.named("value1"),
        ]))
        .named("inner_first")]))
        .named("lol"),
        AbiType::Array(Arc::from(AbiType::Uint(64))).named("third"),
    ]));

    assert_eq!(abi_type, abi_type_manual);

    let from_abi = Test::<Inner>::from_abi(abi).unwrap();
    let from_moved_abi = Test::<Inner>::from_abi(moved_abi).unwrap();

    assert_eq!(from_abi, from_moved_abi);
    assert_eq!(from_abi, test_value);
}

#[derive(IntoAbi, FromAbi, WithAbiType, Eq, PartialEq, Debug, Clone)]
pub struct Test<T> {
    #[abi(name = "kek")]
    pub first: String,
    #[abi(name = "lol")]
    pub second: T,
    pub third: Vec<u64>,
}

#[derive(IntoAbi, FromAbi, WithAbiType, Eq, PartialEq, Debug, Clone)]
pub struct Inner {
    pub inner_first: (String, String),
}

#[derive(IntoAbi, WithAbiType, FromAbi, Debug, Eq, PartialEq, Clone)]
pub struct SomeType<T>(pub T);
