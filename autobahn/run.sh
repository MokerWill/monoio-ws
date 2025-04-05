DIR=$(dirname "$0")

docker run \
    --interactive \
    --tty \
    --rm \
    --volume "${DIR}/config:/config" \
    --volume "${DIR}/reports:/reports" \
    --publish 9001:9001 \
    --name fuzzingserver \
    crossbario/autobahn-testsuite
