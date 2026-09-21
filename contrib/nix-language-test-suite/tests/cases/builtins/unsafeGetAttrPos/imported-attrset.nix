let
  plain = import ./attrset.nix;
  inherited = {inherit (import ./attrset.nix) x;};
in [
  (builtins.unsafeGetAttrPos "x" plain)
  (builtins.unsafeGetAttrPos "x" inherited)
]
