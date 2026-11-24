#!/bin/bash
# Installs Ubuntu packages on a runner, keeping the downloaded files in ~/debs for the next
# run (the job caches that folder): some runners' mirror sends packages at 60 KB/s, and Weston
# alone is 90 MB. apt takes a kept file only where its index lists that very file, and
# downloads the rest.
#   apt.sh <package...>
set -euo pipefail
mkdir -p ~/debs
if compgen -G "$HOME/debs/*.deb" > /dev/null; then
  sudo cp ~/debs/*.deb /var/cache/apt/archives/
fi
sudo apt-get update
sudo apt-get install -y --no-install-recommends -o APT::Keep-Downloaded-Packages=true "$@"
# Kept for the next run: what was downloaded or taken, less what the index no longer lists.
sudo apt-get autoclean -y > /dev/null
rm -rf ~/debs
mkdir ~/debs
if compgen -G "/var/cache/apt/archives/*.deb" > /dev/null; then
  cp /var/cache/apt/archives/*.deb ~/debs/
fi
echo "kept for the next run: $(find ~/debs -name '*.deb' | wc -l) packages, $(du -sh ~/debs | cut -f1)"
