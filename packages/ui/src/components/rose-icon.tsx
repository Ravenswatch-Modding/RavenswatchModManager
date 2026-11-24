import { createLucideIcon } from 'lucide-react';

// Official Lucide Rose geometry (introduced in 0.540.0), backported because
// the workspace bundles 0.469.0. Replace with the package export on upgrade.
// Source: https://lucide.dev/icons/rose
// https://github.com/lucide-icons/lucide/blob/main/icons/rose.svg
// ISC License — Copyright (c) Lucide Contributors 2022.
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
export const RoseIcon = createLucideIcon('Rose', [
  ['path', { d: 'M17 10h-1a4 4 0 1 1 4-4v.534', key: 'petal-left' }],
  [
    'path',
    {
      d: 'M17 6h1a4 4 0 0 1 1.42 7.74l-2.29.87a6 6 0 0 1-5.339-10.68l2.069-1.31',
      key: 'petal-right',
    },
  ],
  [
    'path',
    {
      d: 'M4.5 17c2.8-.5 4.4 0 5.5.8s1.8 2.2 2.3 3.7c-2 .4-3.5.4-4.8-.3-1.2-.6-2.3-1.9-3-4.2',
      key: 'leaf',
    },
  ],
  ['path', { d: 'M9.77 12C4 15 2 22 2 22', key: 'stem' }],
  ['circle', { cx: '17', cy: '8', r: '2', key: 'bloom' }],
]);
