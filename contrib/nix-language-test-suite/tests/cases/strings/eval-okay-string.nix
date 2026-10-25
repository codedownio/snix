# Concatenate string literals
"foo" + "bar"
  # Concatenate paths
  + toString (/a/b + /c/d)
  # Path concatenation normalizes paths
  + toString (/foo/bar + "/../xyzzy/." + "/foo.txt")
  # String concatenation preserves path-like strings without normalizing
  + ("/../foo" + toString /x/y)
  # CppNix 0064599a2: interpret escaped quotes, newlines and backslashes.
  + "escape: \"quote\" \n \\"
  # CppNix 0064599a2: newlines are allowed in strings literals
  + "end
of
line"
  # CppNix 0064599a2: interpolation accepts expressions, including nested interpolation
  + "foo${if true then "b${"a" + "r"}" else "xyzzy"}blaat"
  # CppNix de90fdf90: a dollar is literal as long as it's not followed by "{"
  + "foo$bar"
  # CppNix 467977f20: test dollar literal parsing in different edge cases
  + "$\"$\""
  + "$"
