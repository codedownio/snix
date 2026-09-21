# Test a mix of plain and namespaces inherits in recursive and non-recursive attrsets.
#
# unsafeGetAttrPos should return the positions of inherit,
# not where inheritable attribute is initially defined.
let
  a = 1;
  attrset = {b = 2;};

  namespaced-first = {
    inherit (attrset) b;
    inherit a;
  };
  plain-first = {
    inherit a;
    inherit (attrset) b;
  };
  recursive-namespaced-first = rec {
    inherit (later) b;
    inherit a;
    later = {b = 2;};
  };
  recursive-plain-first = rec {
    inherit a;
    inherit (later) b;
    later = {b = 2;};
  };

  positions = attrs: [
    (builtins.unsafeGetAttrPos "a" attrs).line
    (builtins.unsafeGetAttrPos "a" attrs).column
    (builtins.unsafeGetAttrPos "b" attrs).line
    (builtins.unsafeGetAttrPos "b" attrs).column
  ];
in [
  (positions namespaced-first)
  (positions plain-first)
  (positions recursive-namespaced-first)
  (positions recursive-plain-first)
]
