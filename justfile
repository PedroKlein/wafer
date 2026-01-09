# Use bash for richer scripting (loops, conditionals)
set shell := ["bash", "-c"]

# Config
BROKER_HOST := "mosquitto"
BROKER_PORT := "1883"
CLI := "docker compose run --rm mqttcli"

# Broker lifecycle
mqtt-up:
  docker compose up -d mosquitto

mqtt-logs:
  docker compose logs -f mosquitto

mqtt-down:
  docker compose down

# Run the Rust MQTT subscriber (connects to localhost:1883)
run-mqtt:
  cargo run -- mqtt

# Basic publish/subscribe using efrecon/mqtt-client in Docker
# Usage:
#   just mqtt-pub <topic> <message> <qos>
#   just mqtt-sub <topic> <qos>
mqtt-pub topic message qos:
  {{CLI}} pub -h {{BROKER_HOST}} -p {{BROKER_PORT}} -t "{{topic}}" -m "{{message}}" -q "{{qos}}"

# Retained publish: persists for future subscribers until replaced
# Usage:
#   just mqtt-pub-retained <topic> <message> <qos>
mqtt-pub-retained topic message qos:
  {{CLI}} pub -h {{BROKER_HOST}} -p {{BROKER_PORT}} -t "{{topic}}" -m "{{message}}" -q "{{qos}}" -r

mqtt-sub topic qos:
  {{CLI}} sub -h {{BROKER_HOST}} -p {{BROKER_PORT}} -t "{{topic}}" -q "{{qos}}"

# Stream publisher: send messages at a fixed interval.
# Sends `count` messages; if count is 0, runs indefinitely.
# Message payload: "<prefix>-<seq> @<ts>"
# Usage:
#   just mqtt-stream <topic> <interval_sec> <count> <qos> <prefix>
mqtt-stream topic interval_sec count qos prefix:
  docker compose run --rm --entrypoint /bin/sh mqttcli -c '\
    i=0; \
    while [ "{{count}}" -eq 0 ] || [ "$i" -lt "{{count}}" ]; do \
      ts=$(date +%s); \
      payload="{{prefix}}-$i @$ts"; \
      mqtt pub -h {{BROKER_HOST}} -p {{BROKER_PORT}} -t "{{topic}}" -m "$payload" -q "{{qos}}"; \
      i=$((i+1)); \
      sleep "{{interval_sec}}"; \
    done \
  '

# Burst publisher: sends bursts of messages with a gap between bursts.
# Sends `bursts` groups; each group has `burst_size` messages.
# Usage:
#   just mqtt-burst <topic> <burst_size> <bursts> <gap_sec> <qos> <prefix>
mqtt-burst topic burst_size bursts gap_sec qos prefix:
  docker compose run --rm --entrypoint /bin/sh mqttcli -c '\
    b=0; \
    while [ "$b" -lt "{{bursts}}" ]; do \
      i=0; \
      while [ "$i" -lt "{{burst_size}}" ]; do \
        ts=$(date +%s); \
        payload="{{prefix}}-b${b}-i${i} @$ts"; \
        mqtt pub -h {{BROKER_HOST}} -p {{BROKER_PORT}} -t "{{topic}}" -m "$payload" -q "{{qos}}"; \
        i=$((i+1)); \
      done; \
      b=$((b+1)); \
      if [ "$b" -lt "{{bursts}}" ]; then sleep "{{gap_sec}}"; fi; \
    done \
  '

# JSON publisher: emits simple JSON with timestamp and sequence.
# Usage:
#   just mqtt-json <topic> <interval_sec> <count> <qos>
mqtt-json topic interval_sec count qos:
  docker compose run --rm --entrypoint /bin/sh mqttcli -c '\
    i=0; \
    while [ "{{count}}" -eq 0 ] || [ "$i" -lt "{{count}}" ]; do \
      ts=$(date +%s); \
      payload=$(printf "{\"ts\":%s,\"seq\":%s}" "$ts" "$i"); \
      mqtt pub -h {{BROKER_HOST}} -p {{BROKER_PORT}} -t "{{topic}}" -m "$payload" -q "{{qos}}"; \
      i=$((i+1)); \
      sleep "{{interval_sec}}"; \
    done \
  '