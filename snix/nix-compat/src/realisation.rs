use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::derivation::{OutputName, ParseOutputNameError};
use crate::narinfo::Signature;
use crate::nixhash::{self, NixHash};
use crate::store_path::StorePath;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
#[cfg_attr(
    feature = "serde",
    derive(serde_with::SerializeDisplay, serde_with::DeserializeFromStr)
)]
pub struct DrvOutput {
    pub drv_hash: NixHash,
    pub output_name: OutputName,
}

impl fmt::Display for DrvOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}!{}",
            self.drv_hash.as_nix_lowerhex_string_fmt(),
            self.output_name
        )
    }
}

#[derive(Debug, PartialEq, Error)]
pub enum ParseDrvOutputError {
    #[error("invalid hash in DrvOutput")]
    Hash(
        #[from]
        #[source]
        nixhash::Error,
    ),
    #[error("invalid output name in DrvOutput")]
    OutputName(
        #[from]
        #[source]
        ParseOutputNameError,
    ),
    #[error("missing '!' in DrvOutput '{0}'")]
    InvalidDerivationOutputId(String),
}

impl FromStr for DrvOutput {
    type Err = ParseDrvOutputError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((drv_hash_s, output_name_s)) = s.split_once('!') {
            let drv_hash = NixHash::from_str(drv_hash_s, None)?;
            let output_name = output_name_s.parse()?;
            Ok(DrvOutput {
                drv_hash,
                output_name,
            })
        } else {
            Err(ParseDrvOutputError::InvalidDerivationOutputId(s.into()))
        }
    }
}

#[cfg_attr(
    feature = "serde",
    cfg_eval::cfg_eval,
    serde_with::serde_as,
    derive(serde::Deserialize, serde::Serialize),
    serde(rename_all = "camelCase")
)]
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Realisation {
    pub id: DrvOutput,
    #[cfg_attr(feature = "serde", serde_as(as = "serde_with::DisplayFromStr"))]
    pub out_path: StorePath,
    pub signatures: HashSet<Signature<String>>,
    #[cfg_attr(
        feature = "serde",
        serde_as(as = "BTreeMap<_, serde_with::DisplayFromStr>")
    )]
    pub dependent_realisations: BTreeMap<DrvOutput, StorePath>,
}

pub type DrvOutputs = BTreeMap<DrvOutput, Realisation>;

#[cfg(test)]
mod unittests {
    use rstest::rstest;

    use crate::btree_map;
    use crate::derivation::OutputName;
    use crate::hash_set;
    use crate::nixhash::NixHash;

    use super::{DrvOutput, Realisation};

    #[rstest]
    #[case("sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1!out", DrvOutput {
        drv_hash: NixHash::from_str("sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1", None).unwrap(),
        output_name: OutputName::default(),
    })]
    #[case("sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394!out_put", DrvOutput {
        drv_hash: NixHash::from_str("sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394", None).unwrap(),
        output_name: "out_put".parse().unwrap(),
    })]
    fn parse_drv_output(#[case] value: &str, #[case] expected: DrvOutput) {
        let actual: DrvOutput = value.parse().unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[should_panic = "missing '!' in DrvOutput 'sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394'"]
    #[case("sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394")]
    #[should_panic = "invalid hash in DrvOutput"]
    #[case("sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm39!out")]
    #[should_panic = "invalid output name in DrvOutput"]
    #[case("sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394!out{put")]
    fn parse_drv_output_failure(#[case] value: &str) {
        let actual = value.parse::<DrvOutput>().unwrap_err();
        panic!("{actual}");
    }

    #[rstest]
    #[case(DrvOutput {
        drv_hash: NixHash::from_str("sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1", None).unwrap(),
        output_name: OutputName::default(),
    }, "sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1!out")]
    #[case(DrvOutput {
        drv_hash: NixHash::from_str("sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394", None).unwrap(),
        output_name: OutputName::default(),
    }, "sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1!out")]
    #[case(DrvOutput {
        drv_hash: NixHash::from_str("sha1:y5q4drg5558zk8aamsx6xliv3i23x644", None).unwrap(),
        output_name: "out_put".parse().unwrap(),
    }, "sha1:84983e441c3bd26ebaae4aa1f95129e5e54670f1!out_put")]
    fn display_drv_output(#[case] value: DrvOutput, #[case] expected: &str) {
        assert_eq!(value.to_string(), expected);
    }

    #[cfg(feature = "serde")]
    #[rstest]
    #[case(
        "{\"dependentRealisations\":{\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a496177a9cf410ff61f20015ad!dev\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-dev\",\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a696177a9cf410ff61f20015ad!bin\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-bin\"},\"id\":\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out\",\"outPath\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3\",\"signatures\":[\"cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA==\"]}",
        Realisation {
            id: "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out".parse().unwrap(),
            out_path: "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3".parse().unwrap(),
            signatures: hash_set!["cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA=="],
            dependent_realisations: btree_map![
                "sha256:ba7816bf8f01cfea414140de5dae2223b00361a496177a9cf410ff61f20015ad!dev" => "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-dev",
                "sha256:ba7816bf8f01cfea414140de5dae2223b00361a696177a9cf410ff61f20015ad!bin" => "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-bin",

            ],
        }
    )]
    fn parse_realisation(#[case] value: &str, #[case] expected: Realisation) {
        let actual: Realisation = serde_json::from_str(value).unwrap();
        pretty_assertions::assert_eq!(actual, expected);
    }

    #[cfg(feature = "serde")]
    #[rstest]
    #[case(
        Realisation {
            id: "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out".parse().unwrap(),
            out_path: "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3".parse().unwrap(),
            signatures: hash_set!["cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA=="],
            dependent_realisations: btree_map![
                "sha256:ba7816bf8f01cfea414140de5dae2223b00361a496177a9cf410ff61f20015ad!dev" => "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-dev",
                "sha256:ba7816bf8f01cfea414140de5dae2223b00361a696177a9cf410ff61f20015ad!bin" => "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-bin",

            ],
        },
        "{\"id\":\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out\",\"outPath\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3\",\"signatures\":[\"cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA==\"],\"dependentRealisations\":{\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a496177a9cf410ff61f20015ad!dev\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-dev\",\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a696177a9cf410ff61f20015ad!bin\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-bin\"}}",
    )]
    fn write_realisation(#[case] value: Realisation, #[case] expected: &str) {
        let actual = serde_json::to_string(&value).unwrap();
        pretty_assertions::assert_eq!(actual, expected);
    }
}
