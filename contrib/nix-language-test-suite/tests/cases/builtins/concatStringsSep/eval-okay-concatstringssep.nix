with builtins;

[
  (concatStringsSep "" [])

  # An empty separator joins strings directly
  (concatStringsSep "" [ "foo" "bar" "xyzzy" ])

  # Insert the separator only between elements without leading/trailing separator
  (concatStringsSep ", " [ "foo" "bar" "xyzzy" ])

  # A singleton list adds no separator
  (concatStringsSep ", " [ "foo" ])

  # Concatenation of an empty list produces an empty string
  # even with a non-empty separator
  (concatStringsSep ", " [])
]
