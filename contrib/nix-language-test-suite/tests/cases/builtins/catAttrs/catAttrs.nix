builtins.catAttrs "foo" [
  # foo is present, so it will be selected
  { foo = 21; }

  # Unrelated attributes do not affect selection
  { bar = 23; foo = "+"; }

  # Emtpy attrsets and attrsets without required name are skipped
  { }
  { bar = 12; }

  # Selected values may require evaluation
  { foo = 21 + 0; }

  { foo = "="; }

  # Force the attrset before checking for the attribute
  ({ bar = 13; } // { baz = 89; })

  { foo = 42; bar = 33; }
]
