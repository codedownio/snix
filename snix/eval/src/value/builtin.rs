//! This module implements the runtime representation of a Nix
//! builtin.
//!
//! Builtins are directly backed by Rust code operating on Nix values.

use crate::vm::generators::Generator;

use super::Value;

use std::{
    fmt::{Debug, Display},
    rc::Rc,
};

/// Trait for closure types of builtins.
///
/// Builtins are expected to yield a generator which can be run by the VM to
/// produce the final value.
///
/// Implementors should use the builtins-macros to create these functions
/// instead of handling the argument-passing logic manually.
pub trait BuiltinGen: Fn(Vec<Value>) -> Generator {}
impl<F: Fn(Vec<Value>) -> Generator> BuiltinGen for F {}

/// Function type of synchronous builtins.
///
/// Sync builtins never suspend the VM: all their arguments are strict (the
/// VM forces them before the call, propagating catchables), and their body
/// must not need any VM services (no forcing, calling, coercion, equality).
pub type BuiltinSyncFn = fn(Vec<Value>) -> Result<Value, crate::ErrorKind>;

#[derive(Clone)]
enum BuiltinFunc {
    /// Generator-based builtin, which may suspend the VM.
    Gen(Rc<dyn BuiltinGen>),

    /// Synchronous builtin (see [`BuiltinSyncFn`]).
    Sync(BuiltinSyncFn),
}

#[derive(Clone)]
pub struct BuiltinRepr {
    name: &'static str,
    /// Optional documentation for the builtin.
    documentation: Option<&'static str>,
    arg_count: usize,

    func: BuiltinFunc,

    /// Partially applied function arguments.
    partials: Vec<Value>,
}

pub enum BuiltinResult {
    /// Builtin was not ready to be called (arguments missing) and remains
    /// partially applied.
    Partial(Builtin),

    /// Builtin was called and constructed a generator that the VM must run.
    Called(&'static str, Generator),

    /// A synchronous builtin was fully applied; the VM must force the
    /// arguments (all sync builtin arguments are strict) and invoke the
    /// function directly.
    CalledSync(&'static str, BuiltinSyncFn, Vec<Value>),
}

/// Represents a single built-in function which directly executes Rust
/// code that operates on a Nix value.
///
/// Builtins are the only functions in Nix that have varying arities
/// (for example, `hasAttr` has an arity of 2, but `isAttrs` an arity
/// of 1). To facilitate this generically, builtins expect to be
/// called with a vector of Nix values corresponding to their
/// arguments in order.
///
/// Partially applied builtins act similar to closures in that they
/// "capture" the partially applied arguments, and are treated
/// specially when printing their representation etc.
#[derive(Clone)]
pub struct Builtin(Box<BuiltinRepr>);

impl From<BuiltinRepr> for Builtin {
    fn from(value: BuiltinRepr) -> Self {
        Builtin(Box::new(value))
    }
}

impl Builtin {
    pub fn new<F: BuiltinGen + 'static>(
        name: &'static str,
        documentation: Option<&'static str>,
        arg_count: usize,
        func: F,
    ) -> Self {
        BuiltinRepr {
            name,
            documentation,
            arg_count,
            func: BuiltinFunc::Gen(Rc::new(func)),
            partials: vec![],
        }
        .into()
    }

    /// Construct a synchronous builtin (see [`BuiltinSyncFn`]).
    pub fn new_sync(
        name: &'static str,
        documentation: Option<&'static str>,
        arg_count: usize,
        func: BuiltinSyncFn,
    ) -> Self {
        BuiltinRepr {
            name,
            documentation,
            arg_count,
            func: BuiltinFunc::Sync(func),
            partials: vec![],
        }
        .into()
    }

    pub fn name(&self) -> &'static str {
        self.0.name
    }

    pub fn documentation(&self) -> Option<&'static str> {
        self.0.documentation
    }

    /// Apply an additional argument to the builtin.
    /// After this, [`Builtin::call`] *must* be called, otherwise it may leave
    /// the builtin in an incorrect state.
    pub fn apply_arg(&mut self, arg: Value) {
        self.0.partials.push(arg);

        debug_assert!(
            self.0.partials.len() <= self.0.arg_count,
            "Snix bug: pushed too many arguments to builtin"
        );
    }

    /// Attempt to call a builtin, which will produce a generator if it is fully
    /// applied or return the builtin if it is partially applied.
    pub fn call(self) -> BuiltinResult {
        if self.0.partials.len() == self.0.arg_count {
            match self.0.func {
                BuiltinFunc::Gen(func) => BuiltinResult::Called(self.0.name, func(self.0.partials)),
                BuiltinFunc::Sync(func) => {
                    BuiltinResult::CalledSync(self.0.name, func, self.0.partials)
                }
            }
        } else {
            BuiltinResult::Partial(self)
        }
    }
}

impl Debug for Builtin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "builtin[{}]", self.0.name)
    }
}

impl Display for Builtin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.0.partials.is_empty() {
            f.write_str("<PRIMOP-APP>")
        } else {
            f.write_str("<PRIMOP>")
        }
    }
}

/// Builtins are uniquely identified by their name
impl PartialEq for Builtin {
    fn eq(&self, other: &Self) -> bool {
        self.0.name == other.0.name
    }
}
