#![cfg(test)]
extern crate test_generator;

use json::array;
use std::fs;
use std::path;
use test_generator::test_resources;

use jsonata::JsonAta;

#[test_resources("tests/testsuite/groups/*/*.json")]
fn t(resource: &str) {
    let json = fs::read_to_string(resource).expect("Could not read test case");
    let mut json = json::parse(&json).unwrap();

    if !json.is_array() {
        json = array![json];
    }

    for case in json.members_mut() {
        let expr = if !case["expr"].is_null() {
            case["expr"].to_string()
        } else if !case["expr-file"].is_null() {
            let expr_file = path::Path::new(resource)
                .parent()
                .unwrap()
                .join(case["expr-file"].to_string());
            fs::read_to_string(expr_file).expect("Could not read expr-file")
        } else {
            panic!("No expression")
        };

        //println!("{}", expr);

        let data = if !case["data"].is_null() {
            Some(case["data"].take())
        } else if !case["dataset"].is_null() {
            let dataset =
                fs::read_to_string(format!("tests/testsuite/datasets/{}.json", case["dataset"]))
                    .expect("Could not read dataset file");
            Some(json::parse(&dataset).unwrap().take())
        } else {
            None
        };

        let jsonata = JsonAta::new(&expr);

        match jsonata {
            Ok(jsonata) => {
                let result = jsonata.evaluate(data.as_ref());
                match result {
                    Ok(result) => {
                        if case["undefinedResult"].is_boolean() && case["undefinedResult"] == true {
                            assert_eq!(None, result)
                        } else if !case["result"].is_null() {
                            assert_eq!(case["result"], result.unwrap());
                        }
                    }
                    Err(error) => {
                        assert!(!case["code"].is_null());
                        assert_eq!(case["code"], error.code());
                    }
                }
            }
            Err(error) => {
                println!("{:#?}", error);
                assert!(!case["code"].is_null());
                assert_eq!(case["code"], error.code());
            }
        }

        //println!("{:#?}", jsonata.ast());
    }
}
