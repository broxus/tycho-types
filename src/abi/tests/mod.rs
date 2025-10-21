use std::sync::Arc;

use bytes::Bytes;
use num_bigint::BigInt;

use crate::abi::contract::ContractInitData;
use crate::abi::*;
use crate::models::{AnyAddr, StdAddr};
use crate::prelude::{Cell, CellBuilder, CellFamily, HashBytes, RawDict, Store};

const DEPOOL_ABI: &str = include_str!("depool.abi.json");
const ABI_V2_4: &str = include_str!("contract_v24.abi.json");
const ABI_V2_7: &str = include_str!("wallet_v27.abi.json");

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
    assert_eq!(contract.fields.len(), 4);

    let ContractInitData::PlainFields(fields) = &contract.init_data else {
        panic!("init data is not supported");
    };

    assert_eq!(fields.len(), 2);

    let key = ed25519_dalek::SigningKey::from([1u8; 32]);
    let pubkey = ed25519_dalek::VerifyingKey::from(&key);

    if let Err(e) = contract.encode_init_data(Some(&pubkey), &[NamedAbiValue {
        name: Arc::from("x"),
        value: AbiValue::Int(128, BigInt::from(0)),
    }]) {
        println!("Expected to fail because of unexpected field name. Err: {e:?}");
    }

    let init_values = vec![NamedAbiValue {
        name: Arc::from("d"),
        value: AbiValue::Address(Box::new(AnyAddr::Std(StdAddr::new(
            0,
            HashBytes::default(),
        )))),
    }];

    let cell = contract
        .encode_init_data(Some(&pubkey), init_values.as_ref())
        .unwrap();

    let fields = contract.decode_init_data(cell.as_ref()).unwrap();

    let init = vec![
        NamedAbiValue {
            name: Arc::from("_pubkey"),
            value: AbiValue::FixedBytes(Bytes::copy_from_slice(pubkey.as_bytes())),
        },
        NamedAbiValue {
            name: Arc::from("d"),
            value: AbiValue::Address(Box::new(AnyAddr::Std(StdAddr::new(
                0,
                HashBytes::default(),
            )))),
        },
    ];
    assert_eq!(init, fields);

    let new_init_values = vec![NamedAbiValue {
        name: Arc::from("d"),
        value: AbiValue::Address(Box::new(AnyAddr::Std(StdAddr::new(
            -1,
            HashBytes::default(),
        )))),
    }];

    let new_cell = contract
        .update_init_data(Some(&pubkey), &new_init_values, &cell)
        .unwrap();

    assert_ne!(cell, new_cell);

    let new_fields = contract.decode_init_data(new_cell.as_ref()).unwrap();
    assert_ne!(new_fields, fields);

    let new_init = vec![
        NamedAbiValue {
            name: Arc::from("_pubkey"),
            value: AbiValue::FixedBytes(Bytes::copy_from_slice(pubkey.as_bytes())),
        },
        NamedAbiValue {
            name: Arc::from("d"),
            value: AbiValue::Address(Box::new(AnyAddr::Std(StdAddr::new(
                -1,
                HashBytes::default(),
            )))),
        },
    ];

    assert_eq!(new_init, new_fields);

    let function = contract.find_function_by_id(0x01234567, true).unwrap();
    assert_eq!(function.input_id, 0x01234567);
    assert_eq!(function.name.as_ref(), "has_id");
}

#[test]
fn decode_27_init_data() {
    use crate::models::StateInit;
    use crate::prelude::Boc;
    let abi: &str = r#"
            {
            "ABI version": 2,
            "version": "2.7",
            "header": ["time"],
            "functions": [
                {
                    "name": "constructor",
                    "id": "0x15A038FB",
                    "inputs": [
                        {"name":"walletCode","type":"cell"},
                        {"name":"walletVersion","type":"uint32"},
                        {"name":"sender","type":"address"},
                        {"name":"remainingGasTo","type":"address"}
                    ],
                    "outputs": [
                    ]
                }
            ],
            "getters": [
            ],
            "events": [
            ],
            "fields": [
                {"init":true,"name":"_pubkey","type":"fixedbytes32"},
                {"init":false,"name":"_timestamp","type":"uint64"},
                {"init":false,"name":"_constructorFlag","type":"bool"},
                {"init":true,"name":"root","type":"address"},
                {"init":true,"name":"owner","type":"address"}
            ]
        }
    "#;
    let contract = serde_json::from_str::<Contract>(abi).unwrap();
    let state_init = "te6ccgECEQEAAh8AAgE0AwEBkwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAClpUZkiqXET1LlWmUdabCyx23t4PU866Vze+okEWE5ZIAgBDgBKujOHFR/zpQkRw7J6dw7JSEbq3q0AB7+WeyeICA5ZP8AES/wD0pBP0vPILBAIBIAYFAoTyf4n4aSHbPNMAAY4UgwjXGCD4KMjOzsn5AFj4QvkQ8qje0z8B+EMhufK0IPgjgQPoqIIIG3dAoLnytPhj0x8x8jwODwICxQgHAROyAgw2zz4D/IAgCwNj2OHaiaECAoGuQ64UAfDMRaGmB/SAYfDTUnABuEOOAcYEQ64aP+V4Q8YGA+lIQelD5HkQEAkEaqAVoDj7+EJu4wD4RvJz1NMf+kDU0dD6QNH4SfhKxwUgjxIwIYnHBbMgjogwIds8+EnHBd7fDw4NCgI8joVUcyDbPI4QIMjPhQjOgG/PQMmBAKD7AOJfBNs8DAsALPhK+EP4QsjL/8s/z4PO+EvIzs3J7VQARPhKyM74S88WgQCgz0ASyx/O+CoBzCH7BAHQ7R7tU8nxGAgATPhKyIEBQc9AzgHIzs3J+CrIz4SA9AD0AM+ByfkAyM+KAEDL/8nQAEOAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQADbtRNDT/9M/0wD6QNTR0PpA0fhr+Gr4Zvhj+GIACvhG8uBM";

    let cell = Boc::decode_base64(state_init).unwrap();
    let state_init = cell.parse::<StateInit>().unwrap();

    let data = state_init.data.unwrap();

    let x = contract.decode_init_data(data.as_ref()).unwrap();
    assert_eq!(x.len(), 3);
}

#[test]
fn decode_abi_v27() {
    let contract: Contract = serde_json::from_str::<Contract>(ABI_V2_7).unwrap();
    assert_eq!(contract.abi_version, AbiVersion::V2_7);
    assert_eq!(contract.functions.len(), 14);
    assert_eq!(contract.events.len(), 0);
    assert_eq!(contract.fields.len(), 6);

    let ContractInitData::PlainFields(fields) = &contract.init_data else {
        panic!("init data is not supported");
    };

    assert_eq!(fields.len(), 3);

    let getter = contract.getters.get("get_wallet_data").unwrap();

    assert_eq!(getter.id, 0x17B02);
    assert_eq!(getter.outputs.len(), 4);
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

    let init_data = contract.encode_init_data(Some(&pubkey), &[]).unwrap();

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
