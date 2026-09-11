-- The data this drill proves can come back.
CREATE TABLE orders (
    id          serial PRIMARY KEY,
    customer    text NOT NULL,
    total_cents integer NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);

INSERT INTO orders (customer, total_cents) VALUES
    ('grace', 1299),
    ('alan',  4500),
    ('ada',   8800);
