#!/bin/bash
# Generate traffic for WAFER metrics demo
#
# This script continuously generates messages to /tmp/wafer-input.txt
# for testing the observability stack.
#
# Usage:
#   ./scripts/generate-traffic.sh [OPTIONS]
#
# Options:
#   -r, --rate MSGS    Messages per second (default: 10)
#   -d, --duration SEC Duration in seconds (default: unlimited)
#   -b, --burst N      Initial burst of N messages (default: 100)
#   -h, --help         Show this help message
#
# Examples:
#   ./scripts/generate-traffic.sh              # 10 msg/s indefinitely
#   ./scripts/generate-traffic.sh -r 100       # 100 msg/s
#   ./scripts/generate-traffic.sh -d 60        # Run for 60 seconds
#   ./scripts/generate-traffic.sh -b 1000      # Start with 1000 messages

set -euo pipefail

# Default configuration
RATE=10
DURATION=0  # 0 = unlimited
BURST=100
INPUT_FILE="/tmp/wafer-input.txt"

# Colors
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

show_help() {
    head -20 "$0" | tail -16 | sed 's/^# //' | sed 's/^#//'
    exit 0
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -r|--rate)
            RATE="$2"
            shift 2
            ;;
        -d|--duration)
            DURATION="$2"
            shift 2
            ;;
        -b|--burst)
            BURST="$2"
            shift 2
            ;;
        -h|--help)
            show_help
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

# Calculate sleep interval
SLEEP_INTERVAL=$(echo "scale=4; 1 / $RATE" | bc)

echo -e "${GREEN}╔════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║         WAFER Traffic Generator                    ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "${BLUE}Configuration:${NC}"
echo "  Output file: $INPUT_FILE"
echo "  Rate: $RATE msg/s"
echo "  Initial burst: $BURST messages"
if [ "$DURATION" -gt 0 ]; then
    echo "  Duration: ${DURATION}s"
else
    echo "  Duration: unlimited (Ctrl+C to stop)"
fi
echo ""

# Create/clear input file
> "$INPUT_FILE"

# Initial burst
echo -e "${YELLOW}Sending initial burst of $BURST messages...${NC}"
for i in $(seq 1 "$BURST"); do
    echo "burst-message-$i" >> "$INPUT_FILE"
done
echo -e "${GREEN}✓ Burst complete${NC}"
echo ""

# Continuous generation
echo -e "${YELLOW}Starting continuous traffic at $RATE msg/s...${NC}"
echo "  Press Ctrl+C to stop"
echo ""

counter=$BURST
start_time=$(date +%s)

cleanup() {
    echo ""
    echo -e "${GREEN}✓ Stopped. Generated $counter total messages.${NC}"
    exit 0
}

trap cleanup INT TERM

while true; do
    # Check duration limit
    if [ "$DURATION" -gt 0 ]; then
        elapsed=$(($(date +%s) - start_time))
        if [ "$elapsed" -ge "$DURATION" ]; then
            echo -e "${GREEN}✓ Duration reached. Generated $counter total messages.${NC}"
            exit 0
        fi
    fi

    counter=$((counter + 1))
    echo "message-$counter-$(date +%s%N)" >> "$INPUT_FILE"
    
    # Progress indicator every 100 messages
    if [ $((counter % 100)) -eq 0 ]; then
        echo -e "  Generated $counter messages..."
    fi
    
    sleep "$SLEEP_INTERVAL"
done
