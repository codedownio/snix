mod bfs;
mod descend_to;

pub use bfs::root_to_leaves;
pub use descend_to::descend_to;

use crate::B3Digest;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("unable to lookup directory {0}: {1}")]
    GetFailure(B3Digest, #[source] crate::directoryservice::Error),
    #[error("referenced directory {0} not found")]
    NotFound(B3Digest),
}
