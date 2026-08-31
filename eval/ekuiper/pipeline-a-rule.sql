-- Pipeline A equivalent for eKuiper 2.1.x LTS.
--
-- Registered via seed-pipeline-a.sh; this .sql file is the
-- human-readable reference of what the seed script pushes to the
-- REST API. eKuiper's REST API takes a JSON envelope wrapping the
-- SQL, so this file is documentation, not directly executable by
-- eKuiper.
--
-- Mirrors RFC-008 §D6 Pipeline A: MQTT input → JSON parse →
-- inclusive configured range filter → routed MQTT output.

-- Stream: MQTT source, JSON payload, subscribes to wafer/telemetry.
-- SHARED=true so multiple rules can attach without duplicating the
-- MQTT subscription.
CREATE STREAM wafer_telemetry (
    device_id  STRING,
    temperature FLOAT,
    humidity   FLOAT,
    ts         BIGINT,
    seq        BIGINT
) WITH (
    TYPE       = "mqtt",
    DATASOURCE = "wafer/telemetry",
    FORMAT     = "json",
    SHARED     = "true"
);

-- Passes the five-field input schema through unchanged so the loadgen
-- subscriber computes latency identically to the WAFER/native paths.
SELECT device_id, temperature, humidity, ts, seq
  FROM wafer_telemetry
  WHERE temperature >= 50 AND temperature <= 99999;
