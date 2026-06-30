use crate::nixhash::{Sha256, Sha256Digester};
use crate::store_path::{self, StorePath, StorePathRef};
use bstr::BString;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io;

mod errors;
mod output;
mod output_name;
pub mod outputs;
mod parse_error;
mod parser;
mod validate;
mod write;

#[cfg(test)]
mod tests;

// Public API of the crate.
pub use crate::nixhash::{CAHash, NixHash};
pub use errors::DerivationError;
pub use output::{Output, OutputHash, OutputHashMode};
pub use output_name::{OutputName, ParseOutputNameError};
#[doc(inline)]
pub use outputs::Outputs;
pub use parser::Error as ParserError;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Derivation {
    #[cfg_attr(feature = "serde", serde(rename = "args"))]
    pub arguments: Vec<String>,

    pub builder: String,

    #[cfg_attr(feature = "serde", serde(rename = "env"))]
    pub environment: BTreeMap<String, BString>,

    /// Map from drv path to output names used from this derivation.
    #[cfg_attr(feature = "serde", serde(rename = "inputDrvs"))]
    pub input_derivations: BTreeMap<StorePath, BTreeSet<OutputName>>,

    /// Plain store paths of additional inputs.
    #[cfg_attr(feature = "serde", serde(rename = "inputSrcs"))]
    pub input_sources: BTreeSet<StorePath>,

    /// Maps output names to Output.
    pub outputs: Outputs,

    pub system: String,
}

impl Derivation {
    /// write the Derivation to the given [std::io::Write], in ATerm format.
    ///
    /// The only errors returns are these when writing to the passed writer.
    pub fn serialize(&self, writer: &mut impl std::io::Write) -> Result<(), io::Error> {
        self.serialize_with_replacements(writer, self.input_derivations.iter())
    }

    /// return the ATerm serialization.
    pub fn to_aterm_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        self.serialize(&mut buf).unwrap();
        buf
    }

    /// Parse an Derivation in ATerm serialization, and validate it passes our
    /// set of validations.
    pub fn from_aterm_bytes(b: &[u8]) -> Result<Derivation, parser::Error<&[u8]>> {
        parser::parse(b)
    }

    /// Returns the drv path of a [Derivation] struct.
    ///
    /// The drv path is calculated by invoking [store_path::build_text_path], using
    /// the `name` with a `.drv` suffix as name, all [Derivation::input_sources] and
    /// keys of [Derivation::input_derivations] as references, and the ATerm string of
    /// the [Derivation] as content.
    pub fn calculate_derivation_path(&self, name: &str) -> Result<StorePath, DerivationError> {
        // collect the list of paths from input_sources AND input_derivations
        // into a sorted list of references.
        let mut references: BTreeSet<StorePathRef> = self
            .input_derivations
            .keys()
            .map(StorePath::as_ref)
            .collect();
        references.extend(self.input_sources.iter().map(StorePath::as_ref));

        let drv_name = format!("{name}.drv");
        store_path::build_text_path(
            // append .drv to the name
            &drv_name,
            self.to_aterm_bytes(),
            references,
        )
        .map_err(|err| DerivationError::InvalidDerivationName(drv_name.to_string(), err))
        .map(|sp| sp.to_owned())
    }

    /// Returns the FOD digest, if the derivation is fixed-output, or None if
    /// it's not.
    // NOTE: this is called twice, once when constructing out_output.path is None,
    // it'll later get populated with the path.
    pub fn fod_digest(&self) -> Option<Sha256> {
        if self.outputs.len() != 1 {
            return None;
        }

        let out_output = self.outputs.get(&OutputName::out())?;
        let out_output_hash = out_output.output_hash.as_ref()?;

        Some(store_path::fod_digest(
            out_output_hash.mode == OutputHashMode::Recursive,
            &out_output_hash.hash,
            out_output.path.as_ref().map(|sp| sp.as_ref()),
        ))
    }

    /// Calculates the hash of a derivation modulo fixed-output subderivations.
    ///
    /// This is called `hashDerivationModulo` in nixcpp.
    ///
    /// It returns the sha256 digest of the derivation ATerm representation,
    /// except that:
    ///  -  any input derivation paths have beed replaced "by the result of a
    ///     recursive call to this function" and that
    ///  - for fixed-output derivations the special
    ///    `fixed:out:${algo}:${digest}:${fodPath}` string is hashed instead of
    ///    the A-Term.
    ///
    /// It's up to the caller of this function to provide a (infallible) lookup
    /// function to query the [Derivation::hash_derivation_modulo] of direct
    /// input derivations, by their [StorePathRef].
    /// It will only be called in case the derivation is not a fixed-output
    /// derivation.
    pub fn hash_derivation_modulo<F>(&self, fn_lookup_hash_derivation_modulo: F) -> Sha256
    where
        F: Fn(&StorePathRef) -> Sha256,
    {
        // Fixed-output derivations return a fixed hash.
        // Non-Fixed-output derivations return the sha256 digest of the ATerm
        // notation, but with all input_derivation paths replaced by a recursive
        // call to this function.
        // We call [fn_lookup_hash_derivation_modulo] rather than recursing
        // ourselves, so callers can precompute this.
        self.fod_digest().unwrap_or({
            // For each input_derivation, look up the hash derivation modulo,
            // and replace the derivation path with the hash_derivation_modulo.
            let mut replacements = Vec::from_iter(self.input_derivations.iter().map(
                |(drv_path, output_names)| {
                    (
                        fn_lookup_hash_derivation_modulo(&drv_path.as_ref()),
                        output_names,
                    )
                },
            ));
            // changing the keys changes the order, so we need to sort by keys again
            replacements.sort_by_key(|(k, _output_names)| *k);

            let mut hasher = Sha256Digester::new();
            self.serialize_with_replacements(&mut hasher, replacements.into_iter())
                .unwrap();

            hasher.finalize()
        })
    }

    /// This calculates all output paths of a `Derivation` and updates the struct.
    ///
    /// It requires the outputs to be initially without output paths.
    /// This means, [`Output::path`] needs to be `None` for each output.
    ///
    /// Output path calculation requires knowledge of the
    /// [`hash_derivation_modulo`], which (in case of non-fixed-output
    /// derivations) also requires knowledge of the
    /// [`hash_derivation_modulo`] of input derivations (recursively).
    ///
    /// To avoid recursing and doing unnecessary calculation, we simply
    /// ask the caller of this function to provide the result of the
    /// [`hash_derivation_modulo`] call of the current [`Derivation`],
    /// and leave it up to them to calculate it when needed.
    ///
    /// On completion, [`Output::path`] of each output is set to the calculated output path.
    ///
    /// [`hash_derivation_modulo`]: Derivation::hash_derivation_modulo
    pub fn calculate_output_paths(
        &mut self,
        drv_name: &str,
        hash_derivation_modulo: &Sha256,
    ) -> Result<(), DerivationError> {
        self.outputs
            .calculate_output_paths(drv_name, hash_derivation_modulo)?;

        // The fingerprint and hash differs per output
        for (output_name, output) in self.outputs.iter() {
            self.environment.insert(
                output_name.to_string(),
                output.path.as_ref().unwrap().to_absolute_path().into(),
            );
        }

        Ok(())
    }
}

#[cfg(feature = "async")]
#[allow(dead_code)]
trait DerivationAsyncExt {
    /// Parse an Derivation in ATerm serialization, and validate it passes
    /// our set of validations, from a asynchronous buffered reader.
    /// This is a streaming variant of [Derivation::from_aterm_bytes].
    async fn from_streaming_aterm_bytes<R>(reader: R) -> Result<Derivation, parser::Error<Vec<u8>>>
    where
        R: tokio::io::AsyncBufRead + Unpin + Send;
}

#[cfg(feature = "async")]
impl DerivationAsyncExt for Derivation {
    async fn from_streaming_aterm_bytes<R>(
        mut reader: R,
    ) -> Result<Derivation, parser::Error<Vec<u8>>>
    where
        R: tokio::io::AsyncBufRead + Unpin + Send,
    {
        use tokio::io::AsyncBufReadExt;
        let mut buffer = Vec::new();
        loop {
            let rest = reader.fill_buf().await.unwrap();
            let length = rest.len();

            // We reached EOF, we can stop and return incompleteness.
            if length == 0 {
                return Err(ParserError::Incomplete);
            }

            buffer.extend_from_slice(rest);

            // Parse the so-far internal buffer of reader.
            match parser::parse_streaming(&buffer) {
                (Err(parser::Error::Incomplete), _) => {
                    reader.consume(length);
                    continue;
                }
                (Ok(derivation), leftover) => {
                    // We cannot inline it in the next call because `reader` is mutably borrowed
                    // and has a relationship with the lifetime of `leftover`.
                    let leftover_length = leftover.len();

                    // Well, if we already had consumed the leftovers of the past fetch
                    // while believing we were just parsing incomplete ATerm, there's nothing
                    // we can do about it. The protocol is made this way.
                    if length >= leftover_length {
                        // We still have leftover, let's not consume it.
                        // It's not for us.
                        reader.consume(length - leftover_length);
                    }
                    return Ok(derivation);
                }
                (Err(e), _) => {
                    return Err(e.into());
                }
            }
        }
    }
}
