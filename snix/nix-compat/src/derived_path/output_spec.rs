use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use crate::derivation::{OutputName, ParseOutputNameError};

// FUTUREWORK: reduce the amount of heap allocation needed for this small set of small strings.
/// An output selection spec.
///
/// This is either all outputs (formatted as '*' when displaying or parsing) or
/// a set of [`OutputName`] with the outputs that is to be selected.
///
/// This is used in [`super::DerivedPath`] to perform selection of the outputs to make
/// sure, while building, are valid or substituted.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde_with::DeserializeFromStr, serde_with::SerializeDisplay)
)]
pub enum OutputSpec {
    All,
    Named(BTreeSet<OutputName>),
}

impl OutputSpec {
    pub fn single(output_name: OutputName) -> Self {
        let mut set = BTreeSet::new();
        set.insert(output_name);
        Self::Named(set)
    }
}

impl fmt::Display for OutputSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputSpec::All => f.write_str("*")?,
            OutputSpec::Named(outputs) => {
                let mut it = outputs.iter();
                if let Some(output) = it.next() {
                    write!(f, "{output}")?;
                    for output in it {
                        write!(f, ",{output}")?;
                    }
                }
            }
        }
        Ok(())
    }
}

impl FromStr for OutputSpec {
    type Err = ParseOutputSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "*" {
            Ok(OutputSpec::All)
        } else {
            let mut outputs = BTreeSet::new();
            for (idx, name) in s.split(",").enumerate() {
                let output = name
                    .parse()
                    .map_err(|err| ParseOutputSpecError::OutputName { idx, err })?;
                outputs.insert(output);
            }
            Ok(OutputSpec::Named(outputs))
        }
    }
}

impl From<OutputName> for OutputSpec {
    fn from(output_name: OutputName) -> Self {
        Self::single(output_name)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ParseOutputSpecError {
    #[error("Invalid Output Name at index {idx}")]
    OutputName {
        idx: usize,
        #[source]
        err: ParseOutputNameError,
    },
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use crate::{btree_set, derived_path::OutputSpec};

    #[rstest]
    #[case("*", OutputSpec::All)]
    #[case("out", OutputSpec::Named(btree_set!("out")))]
    #[case("bin,dev,out", OutputSpec::Named(btree_set!("bin", "dev", "out")))]
    #[case::unordered("dev,bin,out", OutputSpec::Named(btree_set!("bin", "dev", "out")))]
    #[case::duplicates("bin,dev,dev", OutputSpec::Named(btree_set!("bin", "dev")))]
    fn parse(#[case] value: &str, #[case] expected: OutputSpec) {
        let actual = value.parse::<OutputSpec>().unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[should_panic]
    #[case("out,bin{n")]
    #[should_panic]
    #[case::too_long(
        "test-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    )]
    fn parse_fail(#[case] value: &str) {
        value.parse::<OutputSpec>().unwrap();
    }

    #[rstest]
    #[case(OutputSpec::All, "*")]
    #[case(OutputSpec::Named(btree_set!("out")), "out")]
    #[case(OutputSpec::Named(btree_set!("bin", "dev", "out")), "bin,dev,out")]
    fn display(#[case] value: OutputSpec, #[case] expected: &str) {
        assert_eq!(value.to_string(), expected);
    }
}
