#!/bin/sh
set -e

# Data directory writable by the application user. Named/bind volumes are
# freshly mounted as root, so when we start as root we normalise ownership
# before dropping privileges.
if [ "$(id -u)" = "0" ]; then
    if [ -e /data ]; then
        chown -R appuser:appuser /data
    fi
    exec runuser -u appuser -- "$@"
fi

# Already running as the app user (or an arbitrary non-root user); just exec.
exec "$@"
