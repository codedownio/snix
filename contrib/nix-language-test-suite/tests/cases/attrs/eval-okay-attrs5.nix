with import ./lib.nix;

let

  as = { x.y.z = 123; a.b.c = 456; };

  # Verify that dashes are permitted in identifiers.
  # see Nix at 95c74eae269b2b9e4bc514581b5caa1d80b54acc.
  bs = { f-o-o.bar = "foo"; };

  # Because `x.y.z or default` is allowed syntax,
  # verify that `or` remains a valid identifier.
  # See Nix at 0a623a10c7e89a80b6dc74445a0ae6240f65723e
  or = x: y: x || y;

in
  # Attribute selections support defaults and chained defaults.
  # See Nix at 0a623a10c7e89a80b6dc74445a0ae6240f65723e
  [ as.x.y.z
    as.foo or "foo"
    as.x.y.bla or as.a.b.c
    as.a.b.c or as.x.y.z
    as.x.y.bla or bs.f-o-o.bar or "xyzzy"
    as.x.y.bla or bs.bar.foo or "xyzzy"
    (123).bla or null.foo or "xyzzy"
    # Backwards compatibility test.
    (fold or [] [true false false])
  ]
