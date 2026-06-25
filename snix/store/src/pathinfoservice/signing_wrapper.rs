//! This module provides a [PathInfoService] implementation that signs narinfos

use super::{PathInfo, PathInfoService};
use crate::pathinfoservice;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use std::path::PathBuf;
use std::sync::Arc;
use tonic::async_trait;

use snix_castore::composition::{CompositionContext, ServiceBuilder};

use nix_compat::narinfo::{Signature, SigningKey, parse_keypair};
use nix_compat::nixbase32;
use tracing::instrument;

/// PathInfoService signing [PathInfo] inserted into it before inserting into
/// the inner [PathInfoService].
///
/// The implementation itself is generic generic (see [nix_compat::narinfo::SigningKey]),
/// but we currently only have [KeyFileSigningPathInfoServiceConfig] to
/// construct with, using a keyfile to sign with.
pub struct SigningPathInfoService<T, S> {
    instance_name: String,
    /// The inner [PathInfoService].
    inner: T,
    /// The key to sign narinfos. Generic over the signer.
    signing_key: SigningKey<S>,
}

impl<T, S> SigningPathInfoService<T, S> {
    pub fn new(instance_name: String, inner: T, signing_key: impl Into<SigningKey<S>>) -> Self {
        Self {
            instance_name,
            inner,
            signing_key: signing_key.into(),
        }
    }
}

#[async_trait]
impl<T, S> PathInfoService for SigningPathInfoService<T, S>
where
    T: PathInfoService,
    S: ed25519::signature::Signer<ed25519::Signature> + Sync + Send,
{
    #[instrument(level = "trace", skip_all, fields(path_info.digest = nixbase32::encode(&digest), instance_name = %self.instance_name))]
    async fn get(&self, digest: [u8; 20]) -> Result<Option<PathInfo>, pathinfoservice::Error> {
        Ok(self.inner.get(digest).await.map_err(Error::Inner)?)
    }

    async fn put(&self, mut path_info: PathInfo) -> Result<PathInfo, pathinfoservice::Error> {
        path_info.signatures.push({
            let mut nar_info = path_info.to_narinfo();
            nar_info.signatures.clear();
            nar_info.add_signature(&self.signing_key);

            let s = nar_info
                .signatures
                .pop()
                .expect("Snix bug: no signature after signing op");
            debug_assert!(
                nar_info.signatures.is_empty(),
                "Snix bug: more than one signature appeared"
            );

            Signature::new(s.name().to_string(), *s.bytes())
        });
        Ok(self.inner.put(path_info).await.map_err(Error::Inner)?)
    }

    fn list(&self) -> BoxStream<'static, Result<PathInfo, pathinfoservice::Error>> {
        self.inner.list().map_err(Error::Inner).err_into().boxed()
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("instantiating from a url is not supported")]
    URLNotSupported,

    #[error("parsing signing key failed: {0}")]
    ParsingSigningKey(#[from] nix_compat::narinfo::SigningKeyError),

    #[error("inner store returned error: {0}")]
    Inner(#[from] pathinfoservice::Error),
}

/// [ServiceBuilder] implementation that builds a [SigningPathInfoService] signing
/// [PathInfo] using a keyfile.
///
/// The keyfile is parsed using [parse_keypair], the expected format is the nix
/// one (see `nix-store --generate-binary-cache-key` for more information).
#[derive(serde::Deserialize)]
pub struct KeyFileSigningPathInfoServiceConfig {
    /// Inner [PathInfoService], will be resolved using a [CompositionContext].
    pub inner: String,
    /// Path to the keyfile in the nix format.
    /// It will be accessed once when constructing the service.
    pub keyfile: PathBuf,
}

impl TryFrom<url::Url> for KeyFileSigningPathInfoServiceConfig {
    type Error = Box<dyn std::error::Error + Send + Sync>;
    fn try_from(_url: url::Url) -> Result<Self, Self::Error> {
        Err(Error::URLNotSupported)?
    }
}

#[async_trait]
impl ServiceBuilder for KeyFileSigningPathInfoServiceConfig {
    type Output = dyn PathInfoService;
    async fn build<'a>(
        &'a self,
        instance_name: &str,
        context: &CompositionContext,
    ) -> Result<Arc<Self::Output>, Box<dyn std::error::Error + Send + Sync>> {
        let inner = context.resolve::<Self::Output>(&self.inner).await?;
        let signing_key = parse_keypair(tokio::fs::read_to_string(&self.keyfile).await?.trim())
            .map_err(Error::ParsingSigningKey)?
            .0;

        Ok(Arc::new(SigningPathInfoService {
            instance_name: instance_name.to_string(),
            inner,
            signing_key,
        }))
    }
}

#[cfg(test)]
/// Helper to construct a [SigningPathInfoService], with the dummy keypair.
/// Not inside the submodule as we also use it
pub fn test_signing_service() -> Arc<dyn PathInfoService> {
    use crate::utils::gen_test_pathinfo_service;

    Arc::new(super::SigningPathInfoService::new(
        "test".into(),
        gen_test_pathinfo_service(),
        parse_keypair(DUMMY_KEYPAIR)
            .expect("DUMMY_KEYPAIR to be valid")
            .0,
    ))
}

#[cfg(test)]
const DUMMY_KEYPAIR: &str = "cache.example.com-1:cCta2MEsRNuYCgWYyeRXLyfoFpKhQJKn8gLMeXWAb7vIpRKKo/3JoxJ24OYa3DxT2JVV38KjK/1ywHWuMe2JEw==";
#[cfg(test)]
const DUMMY_VERIFYING_KEY: &str =
    "cache.example.com-1:yKUSiqP9yaMSduDmGtw8U9iVVd/Coyv9csB1rjHtiRM=";

#[cfg(test)]
mod test {
    use crate::{fixtures::PATH_INFO, pathinfoservice::PathInfoService};
    use nix_compat::narinfo::VerifyingKey;

    #[tokio::test]
    async fn put_and_verify_signature() {
        let svc = super::test_signing_service();

        // Pick a PATH_INFO with 0 signatures…
        assert!(
            PATH_INFO.signatures.is_empty(),
            "PathInfo from fixtures should have no signatures"
        );

        // Asking PathInfoService, it should not be there ...
        assert!(
            svc.get(*PATH_INFO.store_path.digest())
                .await
                .expect("no error")
                .is_none()
        );

        // insert it
        svc.put(PATH_INFO.clone()).await.expect("no error");

        // now it should be there ...
        let path_info = svc
            .get(*PATH_INFO.store_path.digest())
            .await
            .expect("no error")
            .unwrap();

        // Ensure there's a signature now
        let new_sig = path_info
            .signatures
            .last()
            .expect("The retrieved narinfo to be signed")
            .as_ref();

        // verify the new signature against the verifying key
        let verifying_key =
            VerifyingKey::parse(super::DUMMY_VERIFYING_KEY).expect("parsing dummy verifying key");

        // ensure that the new signature is using this key name
        assert_eq!(verifying_key.name(), *new_sig.name());

        assert!(
            verifying_key.verify(&path_info.to_narinfo().fingerprint(), &new_sig),
            "expect signature to be valid"
        );
    }
}
