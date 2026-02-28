#!/bin/bash
# Load test for WAFER /metrics endpoint
#
# This script tests the performance of the Prometheus metrics endpoint
# to ensure it can handle high scrape rates without impacting pipeline performance.
#
# Prerequisites:
#   - WAFER running with metrics enabled (default: localhost:9091)
#   - One of: hey, wrk, or curl installed
#
# Usage:
#   ./scripts/load-test-metrics.sh [OPTIONS]
#
# Options:
#   -u, --url URL        Metrics URL (default: http://localhost:9091/metrics)
#   -c, --concurrency N  Concurrent connections (default: 10)
#   -d, --duration SEC   Test duration in seconds (default: 30)
#   -r, --requests N     Total requests (alternative to duration)
#   -t, --tool TOOL      Force specific tool: hey, wrk, curl (auto-detected)
#   -q, --quiet          Only show summary
#   -h, --help           Show this help message
#
# Examples:
#   ./scripts/load-test-metrics.sh                    # Default 30s test
#   ./scripts/load-test-metrics.sh -d 60 -c 20       # 60s with 20 concurrent
#   ./scripts/load-test-metrics.sh -r 1000           # Fixed 1000 requests
#   ./scripts/load-test-metrics.sh -t wrk            # Force wrk tool

set -euo pipefail

# Default configuration
URL="http://localhost:9091/metrics"
CONCURRENCY=10
DURATION=30
REQUESTS=""
TOOL=""
QUIET=false

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log() {
    if [ "$QUIET" = false ]; then
        echo -e "$1"
    fi
}

error() {
    echo -e "${RED}Error: $1${NC}" >&2
    exit 1
}

show_help() {
    head -30 "$0" | tail -26 | sed 's/^# //' | sed 's/^#//'
    exit 0
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -u|--url)
            URL="$2"
            shift 2
            ;;
        -c|--concurrency)
            CONCURRENCY="$2"
            shift 2
            ;;
        -d|--duration)
            DURATION="$2"
            shift 2
            ;;
        -r|--requests)
            REQUESTS="$2"
            shift 2
            ;;
        -t|--tool)
            TOOL="$2"
            shift 2
            ;;
        -q|--quiet)
            QUIET=true
            shift
            ;;
        -h|--help)
            show_help
            ;;
        *)
            error "Unknown option: $1"
            ;;
    esac
done

# Detect available tool
detect_tool() {
    if [ -n "$TOOL" ]; then
        command -v "$TOOL" &> /dev/null || error "Tool '$TOOL' not found"
        echo "$TOOL"
        return
    fi
    
    if command -v hey &> /dev/null; then
        echo "hey"
    elif command -v wrk &> /dev/null; then
        echo "wrk"
    elif command -v curl &> /dev/null; then
        echo "curl"
    else
        error "No load testing tool found. Install one of: hey, wrk, or curl"
    fi
}

# Check if WAFER is running
check_wafer() {
    log "${BLUE}Checking WAFER metrics endpoint...${NC}"
    if ! curl -sf "$URL" > /dev/null 2>&1; then
        error "Cannot reach $URL - is WAFER running with metrics enabled?"
    fi
    log "${GREEN}✓ WAFER metrics endpoint is reachable${NC}\n"
}

# Run load test with hey
run_hey() {
    log "${BLUE}Running load test with hey...${NC}"
    log "  URL: $URL"
    log "  Concurrency: $CONCURRENCY"
    
    if [ -n "$REQUESTS" ]; then
        log "  Requests: $REQUESTS"
        hey -n "$REQUESTS" -c "$CONCURRENCY" "$URL"
    else
        log "  Duration: ${DURATION}s"
        hey -z "${DURATION}s" -c "$CONCURRENCY" "$URL"
    fi
}

# Run load test with wrk
run_wrk() {
    log "${BLUE}Running load test with wrk...${NC}"
    log "  URL: $URL"
    log "  Concurrency: $CONCURRENCY (threads: 4)"
    log "  Duration: ${DURATION}s"
    
    wrk -t4 -c"$CONCURRENCY" -d"${DURATION}s" "$URL"
}

# Run load test with curl (fallback, less accurate)
run_curl() {
    log "${YELLOW}Using curl fallback (install 'hey' or 'wrk' for better results)${NC}"
    log "${BLUE}Running load test with curl...${NC}"
    log "  URL: $URL"
    log "  Concurrency: $CONCURRENCY"
    
    local total_requests=${REQUESTS:-$((DURATION * 10))}
    log "  Requests: $total_requests"
    
    local start_time=$(date +%s.%N)
    local success=0
    local failed=0
    
    # Simple parallel curl requests
    for i in $(seq 1 "$total_requests"); do
        (
            if curl -sf "$URL" > /dev/null 2>&1; then
                echo "1"
            else
                echo "0"
            fi
        ) &
        
        # Limit concurrency
        if (( i % CONCURRENCY == 0 )); then
            wait
        fi
    done
    wait
    
    local end_time=$(date +%s.%N)
    local elapsed=$(echo "$end_time - $start_time" | bc)
    local rps=$(echo "scale=2; $total_requests / $elapsed" | bc)
    
    echo ""
    echo "Summary:"
    echo "  Total requests: $total_requests"
    echo "  Elapsed time: ${elapsed}s"
    echo "  Requests/sec: $rps"
}

# Main
main() {
    log "${GREEN}╔════════════════════════════════════════════════════╗${NC}"
    log "${GREEN}║       WAFER Metrics Endpoint Load Test             ║${NC}"
    log "${GREEN}╚════════════════════════════════════════════════════╝${NC}\n"
    
    TOOL=$(detect_tool)
    log "Using tool: ${YELLOW}$TOOL${NC}\n"
    
    check_wafer
    
    case $TOOL in
        hey)
            run_hey
            ;;
        wrk)
            run_wrk
            ;;
        curl)
            run_curl
            ;;
        *)
            error "Unknown tool: $TOOL"
            ;;
    esac
    
    echo ""
    log "${GREEN}Load test complete!${NC}"
    log "\n${BLUE}Performance Guidelines:${NC}"
    log "  - Target: > 1000 req/s for metrics endpoint"
    log "  - p99 latency: < 10ms"
    log "  - Error rate: 0%"
}

main
