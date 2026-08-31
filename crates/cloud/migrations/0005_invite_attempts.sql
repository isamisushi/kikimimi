-- Per-device_code invite-code failure counter (gate device activation behind
-- KIKIMIMI_INVITE_CODE for public deployment, see device.rs module docs).
-- Incremented on each wrong `invite_code` POSTed to /activate for a given
-- (still-unexpired) `user_code`; once it reaches the threshold the
-- device_codes row is expired early so the CLI's next `/v1/device/token`
-- poll gets 410 instead of waiting forever for an approval that will never
-- come.
ALTER TABLE device_codes ADD COLUMN IF NOT EXISTS invite_attempts INTEGER NOT NULL DEFAULT 0;
