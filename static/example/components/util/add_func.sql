CREATE OR REPLACE FUNCTION add_two_numbers(a NUMERIC, b NUMERIC)
RETURNS NUMERIC AS $$
BEGIN
    RETURN a + b;
END;
$$ LANGUAGE plpgsql;
