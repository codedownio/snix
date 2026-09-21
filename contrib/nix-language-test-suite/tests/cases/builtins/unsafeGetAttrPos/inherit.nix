let
  # Check inherit inside recursive attrset
  recursive = rec {
    # x's position is where it's inherited, not initially defined
    inherit (later) x;
    later = {x = 2;};
  };

  # Check nested inherit. Outermost x's position should be returned
  x = 1;
  nested = {inherit ({inherit x;}) x;};

  # Check inherit from an anonymous attrset.
  # Outermost x's position should be returned
  anonymous = {inherit ({x = 1;}) x;};

  positions = attrs: [
    (builtins.unsafeGetAttrPos "x" attrs).line
    (builtins.unsafeGetAttrPos "x" attrs).column
  ];
in [
  (positions recursive)
  (positions nested)
  (positions anonymous)
]
