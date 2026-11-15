# Hetzner S3 configuration for snix
# https://docs.hetzner.com/storage/object-storage/getting-started/creating-a-bucket-minio-terraform/

terraform {
  required_providers {
    minio = {
      source = "aminueza/minio"
    }
  }

  backend "s3" {
    endpoints = {
      s3 = "https://s3.dualstack.eu-central-1.amazonaws.com"
    }

    bucket = "snix-tfstate"
    key    = "terraform/snix-hetzner-s3"
    region = "eu-central-1"

    skip_credentials_validation = true
    skip_metadata_api_check = true
    skip_requesting_account_id  = true
  }
}

# Hetzner access keys, not to confuse with the state S3.
variable "access_key" {}

variable "secret_key" {
  sensitive = true
}

provider "minio" {
  minio_server   = "fsn1.your-objectstorage.com"
  minio_user     = "${var.access_key}"
  minio_password = "${var.secret_key}"
  minio_region   = "fsn1"
  minio_ssl      = true
}

resource "minio_s3_bucket" "mimir" {
  bucket         = "snix-mimir"
  acl            = "private"
  object_locking = false
}

resource "minio_s3_bucket" "loki" {
  bucket         = "snix-loki"
  acl            = "private"
  object_locking = false
}

resource "minio_s3_bucket" "tempo" {
  bucket         = "snix-tempo"
  acl            = "private"
  object_locking = false
}
