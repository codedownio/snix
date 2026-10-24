# Bare `${expr}` names work everywhere quoted `"${expr}"` names do.
# See Nix commit f9913f4422d1317af3c6b5aff37ad18b78083eb5
#
# This case is essentially has the same expressions as `eval-okay-dynamic-attrs` but without quotes
let
  aString = "a";

  bString = "b";
in {
  hasAttrs = { a.b = null; } ? ${aString}.b;

  selectAttrs = { a.b = true; }.a.${bString};

  selectOrAttrs = { }.${aString} or true;

  binds = { ${aString}."${bString}c" = true; }.a.bc;

  recBinds = rec { ${bString} = a; a = true; }.b;

  multiAttrs = { ${aString} = true; ${bString} = false; }.a;
}
