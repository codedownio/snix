let {
  as = { x = 123; y = 456; } // { z = 789; } // { z = 987; };

  # Unlike `?` and `.`, these builtins accept attribute names as expressions.
  #
  # Logically that case should be part of builtins/{hasAttr,getAttr},
  # but for consistency with CppNix suite, keep all eval-okay-attr*
  # cases together. See Nix at 8a1ab709a47f0896cb5b47521e31c8c27f88b2b3
  A = "a";
  Z = "z";

  # Test dynamic existence checks and selection with the string names above.
  body = if builtins.hasAttr A as
         then builtins.getAttr A as
         else assert builtins.hasAttr Z as; builtins.getAttr Z as;
}
