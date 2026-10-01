/**
 * Whether `hostname` (as in `URL.hostname`) is this machine or on a private network: `localhost`, loopback,
 * RFC 1918 and link-local IPv4 addresses, IPv6 loopback, unique-local and link-local addresses, or a single-label
 * name (a container or LAN host name such as `api`).
 */
export function isLocalNetworkHost(hostname: string): boolean {
  const host = hostname.toLowerCase().replace(/^\[(.*)\]$/, '$1');
  if (host.includes(':'))
    return host === '::1' || /^f[cd][0-9a-f]{0,2}:/.test(host) || /^fe[89ab][0-9a-f]?:/.test(host);
  if (host === 'localhost' || host.endsWith('.localhost') || !host.includes('.')) return host !== '';
  const octets = host.split('.');
  if (octets.length !== 4 || !octets.every((octet) => /^\d{1,3}$/.test(octet) && Number(octet) <= 255)) return false;
  const [a, b] = octets.map(Number);
  return (
    a === 127 || a === 10 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 169 && b === 254)
  );
}
