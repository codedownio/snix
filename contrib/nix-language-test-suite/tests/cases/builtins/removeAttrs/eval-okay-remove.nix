let {
  attrs = {x = 123; y = 456;};

  # It is not an error for the attribute to be missing
  body = (removeAttrs attrs ["x"]).y;
}
