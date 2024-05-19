-- Resources which can be fetched
CREATE TABLE "resources" (
    -- Unique identifier of the resource
    "id" INTEGER
        NOT NULL
        PRIMARY KEY
        AUTOINCREMENT,
    -- URL of the resource
    "url" VARCHAR(65535)
        NOT NULL
        UNIQUE
);
