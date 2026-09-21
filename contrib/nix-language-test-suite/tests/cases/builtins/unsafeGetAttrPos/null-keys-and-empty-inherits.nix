# Test that empty inherits and null keys don't
# break span collection
let
  attrset = {
    ${null} = 1;
    x = 2;
    inherit;
  };
in {inherit (builtins.unsafeGetAttrPos "x" attrset) line column;}
