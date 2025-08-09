CREATE VIEW user_orders AS
SELECT
    u.id as user_id,
    u.username,
    u.email,
    o.id as order_id,
    o.total_amount,
    o.status,
    o.order_date
FROM users u
LEFT JOIN orders o ON u.id = o.user_id
ORDER BY o.order_date DESC;
