/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  transpilePackages: ['@rsmm/ui', '@rsmm/api-client', '@rsmm/schemas'],
  typedRoutes: true,
  experimental: {
    // Next preloads every route's server bundle when a server starts. On
    // Vercel a function instance often serves one or two routes before it is
    // reclaimed, so that preload was mostly wasted cold-start CPU (Fluid
    // Active CPU is dominated by cold starts here). Routes now load on first
    // use instead.
    preloadEntriesOnStart: false,
  },
  images: {
    // Registry uploads are multi-MB originals; cards request resized WebP via
    // /_next/image (packages/ui thumbnailUrl). Only our own upload hosts.
    remotePatterns: [
      { protocol: 'https', hostname: 's3-ravenswatch.ovilli.de' },
      { protocol: 'https', hostname: 's3-rsmm.me' },
      { protocol: 'https', hostname: 'cdn.rsmm.me' },
      { protocol: 'https', hostname: 'api.rsmm.me' },
    ],
    // Upload keys carry a random suffix and never change content.
    minimumCacheTTL: 31 * 24 * 60 * 60,
  },
  async headers() {
    // Mod pages embed YouTube / Vimeo videos (lib/video-embed.ts). Everything
    // else third-party is AdSense: loader + creatives (googlesyndication /
    // doubleclick), ad-quality beacons (adtrafficquality.google) and the Funding
    // Choices GDPR prompt. A missing directive shows as a blank ad slot or a
    // missing consent prompt, not an error.
    const csp =
      process.env.NODE_ENV === 'production'
        ? [
            "default-src 'self'",
            "script-src 'self' 'unsafe-inline' https://partner.googleadservices.com https://www.googletagservices.com https://adservice.google.com https://pagead2.googlesyndication.com https://*.googlesyndication.com https://*.g.doubleclick.net https://www.google.com https://fundingchoicesmessages.google.com https://*.adtrafficquality.google",
            "style-src 'self' 'unsafe-inline' https://fonts.googleapis.com",
            "connect-src 'self' https://api.rsmm.me https://cdn.rsmm.me https://s3-rsmm.me https://s3-ravenswatch.ovilli.de https://*.googleusercontent.com https://cdn.discordapp.com https://avatars.githubusercontent.com https://pagead2.googlesyndication.com https://*.googlesyndication.com https://*.g.doubleclick.net https://www.google.com https://fundingchoicesmessages.google.com https://*.adtrafficquality.google",
            "img-src 'self' data: blob: https://rsmm.me https://api.rsmm.me https://cdn.rsmm.me https://s3-rsmm.me https://s3-ravenswatch.ovilli.de https://*.googleusercontent.com https://cdn.discordapp.com https://avatars.githubusercontent.com https://pagead2.googlesyndication.com https://*.googlesyndication.com https://*.g.doubleclick.net https://www.google.com https://fundingchoicesmessages.google.com https://*.adtrafficquality.google",
            "font-src 'self' https://fonts.gstatic.com",
            "frame-src 'self' https://www.youtube.com https://player.vimeo.com https://googleads.g.doubleclick.net https://pagead2.googlesyndication.com https://*.googlesyndication.com https://*.g.doubleclick.net https://www.google.com https://fundingchoicesmessages.google.com https://*.adtrafficquality.google",
            "object-src 'none'",
            "form-action 'none'",
            // Belt and braces for the markdown sanitizer: an injected <base> can't
            // repoint relative script URLs, and framing is refused in CSP too.
            "base-uri 'self'",
            "frame-ancestors 'none'",
          ].join('; ')
        : '';
    return [
      {
        // The desktop WebView fetch()es thumbnails into its image cache.
        source: '/_next/image',
        headers: [{ key: 'Access-Control-Allow-Origin', value: '*' }],
      },
      {
        source: '/image-cache-sw.js',
        headers: [{ key: 'Cache-Control', value: 'no-cache' }],
      },
      {
        source: '/(.*)',
        headers: [
          ...(csp ? [{ key: 'Content-Security-Policy', value: csp }] : []),
          {
            key: 'X-Content-Type-Options',
            value: 'nosniff',
          },
          {
            key: 'X-Frame-Options',
            value: 'DENY',
          },
          {
            key: 'Referrer-Policy',
            value: 'strict-origin-when-cross-origin',
          },
        ],
      },
    ];
  },
};

export default nextConfig;
