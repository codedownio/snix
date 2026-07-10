/// This module provides macros which are used in the implementation
/// of the VM for the implementation of repetitive operations.
///
/// This macro simplifies the implementation of arithmetic operations,
/// correctly handling the behaviour on different pairings of number
/// types.
#[macro_export]
macro_rules! arithmetic_op {
    ( $self:ident, $op:tt ) => {{ // TODO: remove
        let b = $self.pop();
        let a = $self.pop();
        let result = fallible!($self, arithmetic_op!(&a, &b, $op));
        $self.push(result);
    }};

    ( $a:expr, $b:expr, $op:tt ) => {{
        match ($a, $b) {
            (Value::Integer(i1), Value::Integer(i2)) => Ok(Value::Integer(i1 $op i2)),
            (Value::Float(f1), Value::Float(f2)) => Ok(Value::Float(f1 $op f2)),
            (Value::Integer(i1), Value::Float(f2)) => Ok(Value::Float(*i1 as f64 $op f2)),
            (Value::Float(f1), Value::Integer(i2)) => Ok(Value::Float(f1 $op *i2 as f64)),

            (v1, v2) => Err(ErrorKind::TypeError {
                expected: "number (either int or float)",
                actual: if v1.is_number() {
                    v2.type_of()
                } else {
                    v1.type_of()
                },
            }),
        }
    }};
}

/// This macro simplifies the implementation of comparison operations.
#[macro_export]
macro_rules! cmp_op {
    ( $vm:ident, $frame:ident, $span:ident, $op:tt ) => {{
        lifted_pop! {
            $vm(b, a) => {
                // Fast path: non-thunk scalar comparisons, mirroring the
                // corresponding arms of Value::nix_cmp_ordering exactly.
                let fast = match (&a, &b) {
                    (Value::Integer(i1), Value::Integer(i2)) => Some(i1.cmp(i2)),
                    (Value::Float(f1), Value::Float(f2)) => Some(f1.total_cmp(f2)),
                    (Value::Integer(i1), Value::Float(f2)) => Some((*i1 as f64).total_cmp(f2)),
                    (Value::Float(f1), Value::Integer(i2)) => Some(f1.total_cmp(&(*i2 as f64))),
                    (Value::String(s1), Value::String(s2)) => Some(s1.cmp(s2)),
                    _ => None,
                };

                if let Some(ordering) = fast {
                    $vm.stack.push(Value::Bool(cmp_op!(@order $op ordering)));
                } else {
                    async fn compare(a: Value, b: Value, co: GenCo) -> Result<Value, ErrorKind> {
                        let a = generators::request_force(&co, a).await;
                        let b = generators::request_force(&co, b).await;
                        let span = generators::request_span(&co).await;
                        let ordering = a.nix_cmp_ordering(b, co, span).await?;
                        match ordering {
                            Err(cek) => Ok(Value::from(cek)),
                            Ok(ordering) => Ok(Value::Bool(cmp_op!(@order $op ordering))),
                        }
                    }

                    let gen_span = $frame.current_span();
                    match $vm.run_generator_over_frame($span, $frame, "compare", gen_span, |co| compare(a, b, co))? {
                        Some(reclaimed) => $frame = reclaimed,
                        None => return Ok(false),
                    }
                }
            }
        }
    }};

    (@order < $ordering:expr) => {
        $ordering == Ordering::Less
    };

    (@order > $ordering:expr) => {
        $ordering == Ordering::Greater
    };

    (@order <= $ordering:expr) => {
        matches!($ordering, Ordering::Equal | Ordering::Less)
    };

    (@order >= $ordering:expr) => {
        matches!($ordering, Ordering::Equal | Ordering::Greater)
    };
}

#[macro_export]
macro_rules! lifted_pop {
    ($vm:ident ($($bind:ident),+) => $body:expr) => {
        {
            $(
                let $bind = $vm.stack_pop();
            )+
            $(
                if $bind.is_catchable() {
                    $vm.stack.push($bind);
                    continue;
                }
            )+
            $body
        }
    }
}
