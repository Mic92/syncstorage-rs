-- fxa_uid: a 16 byte identifier, randomly generated by the fxa server
--    usually a UUID, so presuming a formatted form.
-- fxa_kid: <`mono_num`>-<`client_state`>
--
-- - mono_num: a monotonically increasing timestamp or generation number
--             in hex and padded to 13 digits, provided by the fxa server
-- - client_state: the first 16 bytes of a SHA256 hash of the user's sync
--             encryption key.
--
-- NOTE: DO NOT INCLUDE COMMENTS IF PASTING INTO CONSOLE
--       ALSO, CONSOLE WANTS ONE SPACE BETWEEN DDL COMMANDS

CREATE TABLE user_collections (
  fxa_uid STRING(MAX)  NOT NULL,
  fxa_kid STRING(MAX)  NOT NULL,
  collection_id INT64  NOT NULL,
  modified TIMESTAMP   NOT NULL,
) PRIMARY KEY(fxa_uid, fxa_kid, collection_id);

CREATE TABLE bso (
  fxa_uid STRING(MAX)  NOT NULL,
  fxa_kid STRING(MAX)  NOT NULL,
  collection_id INT64  NOT NULL,
  id STRING(64)        NOT NULL,
  sortindex INT64,
  payload STRING(MAX)  NOT NULL,
  modified TIMESTAMP   NOT NULL,
  expiry TIMESTAMP     NOT NULL,
)    PRIMARY KEY(fxa_uid, fxa_kid, collection_id, id),
  INTERLEAVE IN PARENT user_collections ON DELETE CASCADE;

    CREATE INDEX BsoModified
        ON bso(fxa_uid, fxa_kid, collection_id, modified DESC, expiry),
INTERLEAVE IN user_collections;

    CREATE INDEX BsoExpiry
           ON bso(expiry);

CREATE TABLE collections (
  id INT64          NOT NULL,
  name STRING(32)   NOT NULL,
) PRIMARY KEY(id);

    CREATE UNIQUE INDEX CollectionName
        ON collections(name);

INSERT INTO collections (id, name) VALUES
    ( 1, "clients"),
    ( 2, "crypto"),
    ( 3, "forms"),
    ( 4, "history"),
    ( 5, "keys"),
    ( 6, "meta"),
    ( 7, "bookmarks"),
    ( 8, "prefs"),
    ( 9, "tabs"),
    (10, "passwords"),
    (11, "addons"),
    (12, "addresses"),
    (13, "creditcards");

CREATE TABLE batches (
  fxa_uid STRING(MAX)  NOT NULL,
  fxa_kid STRING(MAX)  NOT NULL,
  collection_id INT64  NOT NULL,
  batch_id STRING(MAX) NOT NULL,
  expiry TIMESTAMP     NOT NULL,
)    PRIMARY KEY(fxa_uid, fxa_kid, collection_id, batch_id),
  INTERLEAVE IN PARENT user_collections ON DELETE CASCADE;

CREATE TABLE batch_bso (
  fxa_uid STRING(MAX)  NOT NULL,
  fxa_kid STRING(MAX)  NOT NULL,
  collection_id INT64  NOT NULL,
  batch_id STRING(MAX) NOT NULL,
  id STRING(64)        NOT NULL,

  sortindex INT64,
  payload STRING(MAX),
  ttl INT64,
)    PRIMARY KEY(fxa_uid, fxa_kid, collection_id, batch_id, id),
  INTERLEAVE IN PARENT batches ON DELETE CASCADE;

-- batch_bso's bso fields are nullable as the batch upload may or may
-- not set each individual field of each item. Also note that there's
-- no "modified" column because the modification timestamp gets set on
-- batch commit.
