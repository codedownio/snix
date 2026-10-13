let {
  # Attribute-set updates are right-biased, so the final value of `z` is 987.
  as = { x = 123; y = 456; } // { z = 789; } // { z = 987; };

  # Test attribute existence with `?`, then select the existing attribute.
  body = if as ? a then as.a else assert as ? z; as.z;
}
