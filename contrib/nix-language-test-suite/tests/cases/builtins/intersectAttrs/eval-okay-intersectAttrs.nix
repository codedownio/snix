let
  alphabet =
  { a = "a";
    b = "b";
    c = "c";
    d = "d";
    e = "e";
    f = "f";
    g = "g";
    h = "h";
    i = "i";
    j = "j";
    k = "k";
    l = "l";
    m = "m";
    n = "n";
    o = "o";
    p = "p";
    q = "q";
    r = "r";
    s = "s";
    t = "t";
    u = "u";
    v = "v";
    w = "w";
    x = "x";
    y = "y";
    z = "z";
  };
  foo = {
    inherit (alphabet) f o b a r z q u x;
    aa = throw "aa";
  };
  alphabetFail = builtins.mapAttrs throw alphabet;
in
# These tests were added in Nix at 336908c. The commit contains lookup performance optimization,
# so some tests might be redundant from the pure language semantics point of view, but make sense from
# the performance optimization verification point.
[
  # Disjoint names produce an empty set without forcing either value
  (builtins.intersectAttrs { a = abort "l1"; } { b = abort "r1"; })

  # Ignore the left value and return the matching right value
  (builtins.intersectAttrs { a = abort "l2"; } { a = 1; })

  # A larger left set must not force its values
  (builtins.intersectAttrs alphabetFail { a = 1; })

  # The cases below (except the last one) seem to test
  # current CppNix lookup implementation rather than pure language
  # semantics

  # A larger right set still supplies the result values
  (builtins.intersectAttrs  { a = abort "laa"; } alphabet)

  # Match a key in the middle of a larger left set
  (builtins.intersectAttrs alphabetFail { m = 1; })

  # Match a key in the middle of a larger right set
  (builtins.intersectAttrs  { m = abort "lam"; } alphabet)

  # Match the next key in a larger left set
  (builtins.intersectAttrs alphabetFail { n = 1; })

  # Match the next key in a larger right set
  (builtins.intersectAttrs  { n = abort "lan"; } alphabet)

  # Select multiple keys from a smaller right set
  (builtins.intersectAttrs alphabetFail { n = 1; p = 2; })

  # Select multiple keys from a larger right set
  (builtins.intersectAttrs  { n = abort "lan2"; p = abort "lap"; } alphabet)

  # This case repeats the case before the previous one
  (builtins.intersectAttrs alphabetFail { n = 1; p = 2; })

  # This case repeats the case before the previous one
  (builtins.intersectAttrs  { n = abort "lan2"; p = abort "lap"; } alphabet)

  # Equal-sized sets still take all values from the right
  (builtins.intersectAttrs alphabetFail alphabet)

  # - With equal shared values, swapping sets gives equal results
  # - the unshared throwing value (foo.aa = throw "aa") stays lazy.
  (builtins.intersectAttrs alphabet foo == builtins.intersectAttrs foo alphabet)
]
