# Dynamic paths with the same static leading part are merged into one nested set.
# See Nix commit 6f3a51809a2603574a16573bd46b95e4ff5233bd
{ a."${"b"}" = true; a."${"c"}" = false; }.a.b
