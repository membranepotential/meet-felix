// The custom-domain front door: a Cloudflare Pages project whose only job is to forward every
// request (including WebSocket upgrades) to the meet-felix Worker through a service binding.
// Pages accepts custom domains whose DNS lives outside Cloudflare; Workers do not
// (docs/research/10-custom-hostname-external-dns.md).
export default {
  fetch(request, env) {
    return env.MEET.fetch(request);
  },
};
