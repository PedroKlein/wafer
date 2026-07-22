-- Pipeline A equivalent for eKuiper 2.1.x LTS.
--
-- Registered via seed-pipeline-a.sh; this .sql file is the
-- human-readable reference of what the seed script pushes to the
-- REST API. eKuiper's REST API takes a JSON envelope wrapping the
-- SQL, so this file is documentation, not directly executable by
-- eKuiper.
--
-- Mirrors RFC-008 §D6 Pipeline A: MQTT input → JSON parse →
-- temperature > 50 filter → routed MQTT output.

-- Stream: MQTT source, JSON payload, subscribes to wafer/telemetry.
-- SHARED=true so multiple rules can attach without duplicating the
-- MQTT subscription.
CREATE STREAM wafer_telemetry (
    sequence   BIGINT,
    intended_ns BIGINT,
    temperature FLOAT
) WITH (
    TYPE       = "mqtt",
    DATASOURCE = "wafer/telemetry",
    FORMAT     = "json",
    SHARED     = "true"
);

-- Rule: temperature > 50 → publish to wafer/telemetry/hot.
-- Passes sequence + intended_ns through unchanged so the loadgen
-- subscriber can compute latency the same way it does against WAFER.
SELECT sequence, intended_ns, temperature
  FROM wafer_telemetry
  WHERE temperature > 50;
