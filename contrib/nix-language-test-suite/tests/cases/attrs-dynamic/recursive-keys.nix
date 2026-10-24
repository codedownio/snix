rec {
  val = 21;
  # Dynamic keys work in recursive sets and their values can reference static
  # bindings from the same set
  ${"foo" + "bar"} = 42;
  ${"bar" + "baz"} = val * 2;
}
