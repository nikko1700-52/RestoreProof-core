-- Stands in for whatever your backup job produces.
--
-- PostgreSQL executes every .sql file placed in /docker-entrypoint-initdb.d
-- the first time it initialises a data directory, so restoring this file and
-- starting the container is a faithful miniature of "restore the dump, start
-- the database".
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

CREATE TABLE customers (
    id    serial PRIMARY KEY,
    email text NOT NULL UNIQUE
);

INSERT INTO customers (email) VALUES
    ('grace@example.test'),
    ('alan@example.test'),
    ('ada@example.test');
