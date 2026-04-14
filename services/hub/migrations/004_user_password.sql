-- Add password hash to users for local auth
ALTER TABLE users ADD COLUMN IF NOT EXISTS password_hash TEXT;
