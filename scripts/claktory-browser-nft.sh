#!/bin/sh
# Claktory's browser (#435): loads its firewall, replacing only the table
# inet claktory_browser (/etc/claktory-browser.nft) in one transaction, then
# lets the browser's user ask the resolvers in /etc/resolv.conf, port 53 only.
# Installed as /usr/local/sbin/claktory-browser-nft by browser-setup-linux.sh
# (run by claktory-browser-nft.service) and in the Mac image (run by its
# entrypoint).
set -e
PATH=/usr/sbin:/usr/bin:/sbin:/bin
printf 'table inet claktory_browser {}\ndelete table inet claktory_browser\ninclude "/etc/claktory-browser.nft"\n' | nft -f -
for ns in $(awk '$1 == "nameserver" { print $2 }' /etc/resolv.conf 2>/dev/null); do
  case "$ns" in
    *%*) ;;
    *:*) nft add element inet claktory_browser dns6 "{ $ns }" || true ;;
    *) nft add element inet claktory_browser dns4 "{ $ns }" || true ;;
  esac
done
