# Regression: the second argument must be type-checked even when the
# first argument is an empty attrset. C++ Nix errors here.
builtins.intersectAttrs { } null
