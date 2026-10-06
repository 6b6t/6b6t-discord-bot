-- Eligibility-gated button polls. Applied by `polls::store::migrate` to the bot's
-- link database; every statement is idempotent.

CREATE TABLE IF NOT EXISTS polls (
  poll_id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
  channel_id VARCHAR(64) NOT NULL,
  message_id VARCHAR(64) NULL,
  title VARCHAR(256) NOT NULL,
  options_json TEXT NOT NULL,
  -- The expression exactly as staff typed it; never shown to players.
  requires_expr VARCHAR(512) NOT NULL,
  -- Internal: the class parameters that were in force when the poll started.
  rule_json TEXT NOT NULL,
  -- The freeze point (UTC ms). Class data at or after it is never read for this poll.
  cutoff_ms BIGINT NOT NULL,
  window_days INT NOT NULL,
  starts_at BIGINT NOT NULL,
  ends_at BIGINT NOT NULL,
  status VARCHAR(16) NOT NULL,
  eligible_count INT NOT NULL DEFAULT 0,
  dirty TINYINT NOT NULL DEFAULT 0,
  last_render_ms BIGINT NOT NULL DEFAULT 0,
  finalized TINYINT NOT NULL DEFAULT 0,
  closed_at BIGINT NULL,
  result_json TEXT NULL,
  created_by VARCHAR(64) NOT NULL,
  created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  INDEX idx_polls_status (status, ends_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- The frozen snapshot: one row per person allowed to vote.
CREATE TABLE IF NOT EXISTS poll_eligible (
  poll_id BIGINT UNSIGNED NOT NULL,
  discord_id VARCHAR(64) NOT NULL,
  uuid CHAR(36) NOT NULL,
  person_key VARCHAR(64) NOT NULL,
  PRIMARY KEY (poll_id, discord_id),
  UNIQUE KEY unique_poll_person (poll_id, person_key)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS poll_votes (
  poll_id BIGINT UNSIGNED NOT NULL,
  voter_discord_id VARCHAR(64) NOT NULL,
  option_idx TINYINT UNSIGNED NOT NULL,
  voted_at BIGINT NOT NULL,
  changed_at BIGINT NULL,
  PRIMARY KEY (poll_id, voter_discord_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Counters only: no ids, so denials cannot be traced to people.
CREATE TABLE IF NOT EXISTS poll_denials (
  poll_id BIGINT UNSIGNED NOT NULL,
  reason_code VARCHAR(32) NOT NULL,
  n INT UNSIGNED NOT NULL DEFAULT 0,
  PRIMARY KEY (poll_id, reason_code)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
