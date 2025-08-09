CREATE TABLE user_activity (
    id SERIAL PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    activity_type VARCHAR(50) NOT NULL,
    activity_data JSONB,
    page_url VARCHAR(500),
    session_id VARCHAR(100),
    ip_address INET,
    user_agent TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_user_activity_user_id ON user_activity(user_id);
CREATE INDEX idx_user_activity_type ON user_activity(activity_type);
CREATE INDEX idx_user_activity_created_at ON user_activity(created_at);
CREATE INDEX idx_user_activity_session_id ON user_activity(session_id);

-- View for recent user activity
CREATE VIEW recent_user_activity AS
SELECT
    ua.id,
    u.username,
    ua.activity_type,
    ua.page_url,
    ua.created_at,
    ua.session_id
FROM user_activity ua
JOIN users u ON ua.user_id = u.id
WHERE ua.created_at >= NOW() - INTERVAL '24 hours'
ORDER BY ua.created_at DESC;

-- Function to track user login
CREATE OR REPLACE FUNCTION track_user_login(
    p_user_id INTEGER,
    p_ip_address INET,
    p_user_agent TEXT
)
RETURNS VOID AS $$
BEGIN
    INSERT INTO user_activity (user_id, activity_type, ip_address, user_agent)
    VALUES (p_user_id, 'login', p_ip_address, p_user_agent);
END;
$$ LANGUAGE plpgsql;
