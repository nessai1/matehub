-- Add icon customization to channels
ALTER TABLE channels ADD COLUMN IF NOT EXISTS icon_id TEXT;
ALTER TABLE channels ADD COLUMN IF NOT EXISTS icon_color TEXT;
ALTER TABLE channels ADD COLUMN IF NOT EXISTS icon_image_url TEXT;
