#!/bin/sh
set -eu
package_version=$(sed -n '/^\[package\]/,/^\[/s/^version = "\([^"]*\)"/\1/p' Cargo.toml)
test -n "$package_version"
if [ -n "${CI_COMMIT_TAG:-}" ]; then
    release_version=${CI_COMMIT_TAG#v}
    printf '%s\n' "$release_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || {
        echo 'Release tag must be X.Y.Z or vX.Y.Z'; exit 1;
    }
    test "$release_version" = "$package_version" || {
        echo 'Release tag differs from Cargo.toml'; exit 1;
    }
fi
if [ "${1:-}" = '--check' ]; then
    printf '%s\n' "$package_version"
    exit 0
fi
: "${AKUNKEUANGAN_IMAGE:?}" "${CI_COMMIT_SHA:?}"
source_image="$AKUNKEUANGAN_IMAGE:$CI_COMMIT_SHA"
publish_tag() {
    docker tag "$source_image" "$AKUNKEUANGAN_IMAGE:$1"
    docker push "$AKUNKEUANGAN_IMAGE:$1"
}
docker push "$source_image"
publish_tag "postgres-$CI_COMMIT_SHA"
if [ -n "${CI_COMMIT_BRANCH:-}" ]; then
    publish_tag "postgres-$CI_COMMIT_REF_SLUG"
fi
if [ -n "${CI_COMMIT_TAG:-}" ]; then
    publish_tag "$release_version"
    publish_tag "postgres-$release_version"
    if [ "$CI_COMMIT_TAG" != "$release_version" ]; then
        publish_tag "$CI_COMMIT_TAG"
    fi
fi
# A stable release and default-branch builds update the rolling aliases.
if [ -n "${CI_COMMIT_TAG:-}" ] || [ "${CI_COMMIT_BRANCH:-}" = "$CI_DEFAULT_BRANCH" ]; then
    publish_tag latest
    publish_tag postgres-latest
fi
