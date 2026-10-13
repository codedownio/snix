let

  config =
    {
      # Dotted definitions are shorthand for nested attribute sets and merge
      # definitions that share an intermediate path.
      # See Nix at d407d572fdc72f4eb14cc0f37d7d61446425b663.
      services.sshd.enable = true;
      services.sshd.port = 22;
      services.httpd.port = 80;
      hostName = "itchy";
      a.b.c.d.e.f.g.h.i.j.k.l.m.n.o.p.q.r.s.t.u.v.w.x.y.z = "x";
      foo = {
        a = "a";
        b.c = "c";
      };
    };

in
  if config.services.sshd.enable
  then "foo ${toString config.services.sshd.port} ${toString config.services.httpd.port} ${config.hostName}"
       + "${config.a.b.c.d.e.f.g.h.i.j.k.l.m.n.o.p.q.r.s.t.u.v.w.x.y.z}"
       + "${config.foo.a}"
       + "${config.foo.b.c}"
  else "bar"
