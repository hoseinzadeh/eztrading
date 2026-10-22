-- Schema reconstructed from the queries in servers/com/src/db, servers/jobsrv and servers/srvman.
-- Load with:  mysql -uroot -p < db/schema.sql
CREATE DATABASE IF NOT EXISTS eztrading CHARACTER SET utf8mb4;
USE eztrading;

CREATE TABLE IF NOT EXISTS users (
  id          INT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
  name        VARCHAR(100) NOT NULL,
  lastname    VARCHAR(100) NOT NULL DEFAULT '',
  email       VARCHAR(255) NOT NULL UNIQUE,
  phonenumber VARCHAR(40)  NOT NULL DEFAULT '',
  joined      DATETIME     NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS permissions (
  user_id  INT UNSIGNED NOT NULL PRIMARY KEY,
  follow   TINYINT UNSIGNED NOT NULL DEFAULT 1,
  simulate TINYINT UNSIGNED NOT NULL DEFAULT 1,
  develop  TINYINT UNSIGNED NOT NULL DEFAULT 1,
  earn     TINYINT UNSIGNED NOT NULL DEFAULT 0,
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

-- Column order matters: the code reads `SELECT * FROM plugins` positionally.
CREATE TABLE IF NOT EXISTS plugins (
  id        INT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
  user_id   INT UNSIGNED NOT NULL,
  name      VARCHAR(100) NOT NULL,
  source    MEDIUMTEXT   NOT NULL,
  machines  MEDIUMTEXT   NULL,
  lib       LONGBLOB     NULL,
  version   VARCHAR(40)  NOT NULL DEFAULT '0.1.0',
  price     FLOAT        NULL,
  date      DATE         NOT NULL DEFAULT (CURRENT_DATE),
  published TINYINT(1)   NOT NULL DEFAULT 0,
  UNIQUE KEY user_plugin (user_id, name),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS subscriptions (
  user_id   INT UNSIGNED NOT NULL,
  plugin_id INT UNSIGNED NOT NULL,
  renew     TINYINT(1)   NOT NULL DEFAULT 0,
  expire    DATE         NOT NULL,
  PRIMARY KEY (user_id, plugin_id),
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
  FOREIGN KEY (plugin_id) REFERENCES plugins(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS bots (
  id        INT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
  user_id   INT UNSIGNED NOT NULL,
  plugin_id INT UNSIGNED NOT NULL,
  machine   MEDIUMTEXT   NOT NULL,
  server_id INT          NULL,
  FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
  FOREIGN KEY (plugin_id) REFERENCES plugins(id) ON DELETE CASCADE
);

-- A development user (user id 1) so a fresh cluster can be tried straight away.
INSERT IGNORE INTO users (id, name, lastname, email) VALUES (1, 'Dev', 'User', 'dev@example.com');
INSERT IGNORE INTO permissions (user_id, follow, simulate, develop, earn) VALUES (1, 1, 1, 1, 1);
