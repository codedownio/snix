let

  as = { x.y.z = 123; a.b.c = 456; };

  bs = null;

in
  # Nested paths are supported by `?`; missing intermediate attributes and
  # non-attribute left-hand side produce false.
  # See Nix at 56370378023fc84eb0153b991f4138f6acd011e3
  [ (as ? x) (as ? y) (as ? x.y.z) (as ? x.y.z.a) (as ? x.y.a) (as ? a.b.c) (bs ? x) (bs ? x.y.z) ]
