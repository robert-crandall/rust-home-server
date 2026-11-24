-- Allow identity-provider users (e.g. Cloudflare Access) to exist without a local password.
ALTER TABLE users ALTER COLUMN password_hash DROP NOT NULL;
