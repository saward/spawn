CREATE VIEW daily_sales_summary AS
SELECT
    DATE(o.order_date) as sale_date,
    COUNT(o.id) as total_orders,
    COUNT(DISTINCT o.user_id) as unique_customers,
    SUM(o.total_amount) as total_revenue,
    AVG(o.total_amount) as average_order_value,
    MIN(o.total_amount) as min_order_value,
    MAX(o.total_amount) as max_order_value
FROM orders o
WHERE o.status IN ('completed', 'shipped', 'delivered')
GROUP BY DATE(o.order_date)
ORDER BY sale_date DESC;

-- Helper function to get sales for a specific date
CREATE OR REPLACE FUNCTION get_daily_sales(target_date DATE)
RETURNS TABLE(
    orders_count BIGINT,
    revenue NUMERIC,
    avg_order NUMERIC
) AS $$
BEGIN
    RETURN QUERY
    SELECT
        COUNT(o.id),
        COALESCE(SUM(o.total_amount), 0),
        COALESCE(AVG(o.total_amount), 0)
    FROM orders o
    WHERE DATE(o.order_date) = target_date
    AND o.status IN ('completed', 'shipped', 'delivered');
END;
$$ LANGUAGE plpgsql;
