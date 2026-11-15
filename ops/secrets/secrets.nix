# This file is read by agenix standalone, to know which audiences to (re)encrypt secrets to.

let
  flokli = [
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPTVTXOutUZZjXLB0lUSgeKcSY/8mxKkC0ingGK1whD2 flokli"
  ];

  picnoir = [
    "age1r6gg90uwpacmkkm53zn5jgs38p2f3uam53w24q0g0ttyw0tcsups9m0s3l"
  ];

  gerrit01 = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIN+RCLAExaM5EC70UsCPMtDT1Cfa80Ux/vex95fLk9S4 root@gerrit01";
  public01 = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICzB7bqXWcv+sVokySvj1d74zRlVLSNqBw7/OY3c7QYd root@public01";
  build03 = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG8G+x82sCebiH35kmWjWT7js3rOMiJoUNu7DjYL6B6j root@build03";

  meta01 = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINj2csTShq5PsmB/T0596TASyf7VImD4592HEqaYHgKh root@meta01";
  snixCache = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIF5bYa2cxaTUpweycBNv1YNDNOFPt4LR9I9bEm2X6GdH root@snix-cache";

  superadmins = flokli ++ picnoir;

  allDefault.publicKeys = superadmins ++ [
    gerrit01
    public01
    build03
    meta01
    snixCache
  ];
  terraform.publicKeys = superadmins;
  gerrit01Default.publicKeys = superadmins ++ [ gerrit01 ];
  public01Default.publicKeys = superadmins ++ [ public01 ];
  builderDefault.publicKeys = superadmins ++ [
    build03
  ];
  meta01Default.publicKeys = superadmins ++ [ meta01 ];
  ciDefault.publicKeys = superadmins ++ [
    gerrit01
    build03
  ];
in
{
  "grafana-agent-password.age" = allDefault;

  "restic-repository-password.age" = allDefault;
  "restic-bucket-credentials.age" = allDefault;

  "keycloak-db-password.age" = public01Default;

  "buildkite-api-proxy-token.age" = gerrit01Default;
  "gerrit-oauth-secret.age" = gerrit01Default;
  "gerrit-replication-key.age" = gerrit01Default;
  "gerrit-sendemail-smtp-pass.age" = gerrit01Default;
  "gerrit-autosubmit.age" = gerrit01Default;

  "forgejo-oauth-secret.age" = public01Default;
  "forgejo-smtp-passwd.age" = public01Default;

  "grafana-oauth-secret.age" = public01Default;

  "ci-cache-bucket-credentials.age" = public01Default;

  "binary-cache-key.age" = builderDefault;
  "buildkite-agent-token.age" = builderDefault;
  "buildkite-ssh-private-key.age" = builderDefault;
  "buildkite-besadii-config.age" = ciDefault;
  "buildkite-graphql-token.age" = builderDefault;

  "metrics-push-htpasswd.age" = meta01Default;
  "alertmanager-irc-relay-environment.age" = meta01Default;
  "irccat-secrets.age" = meta01Default;
  "loki-environment.age" = meta01Default;
  "mimir-environment.age" = meta01Default;
  "mimir-webhook-url.age" = meta01Default;

  "tf-dns.age" = terraform;
  "tf-keycloak.age" = terraform;
  "tf-hcloud.age" = terraform;
  "tf-hetzner-s3.age" = terraform;
  "tf-buildkite.age" = terraform;

  "mtls-build03.infra.snix.dev.age" = {
    publicKeys = [ build03 ];
  };
}
