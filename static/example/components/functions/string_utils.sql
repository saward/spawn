CREATE OR REPLACE FUNCTION capitalize_words(input_text TEXT)
RETURNS TEXT AS $$
BEGIN
    RETURN INITCAP(input_text);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION clean_phone(phone_number TEXT)
RETURNS TEXT AS $$
BEGIN
    RETURN REGEXP_REPLACE(phone_number, '[^0-9]', '', 'g');
END;
$$ LANGUAGE plpgsql;
