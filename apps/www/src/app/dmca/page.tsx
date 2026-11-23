import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@rsmm/ui';
import type { Metadata } from 'next';
import Link from 'next/link';

export const metadata: Metadata = {
  title: 'Content Policy & Copyright (DMCA)',
  description:
    'Acceptable-use rules for the Ravenswatch Mod Manager registry and how to report copyright-infringing content for takedown.',
  alternates: { canonical: '/dmca' },
};

export default function DmcaPage() {
  return (
    <main className="container mx-auto px-6 py-16 animate-page-in">
      <Card className="mx-auto max-w-3xl grimoire-card">
        <CardHeader>
          <CardTitle>Content Policy &amp; Copyright</CardTitle>
          <CardDescription>
            What may be uploaded to the registry, and how to report infringing content.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6 text-sm text-muted-foreground">
          <section className="space-y-2">
            <h2 className="text-lg font-semibold text-foreground">1. User-generated content</h2>
            <p>
              Mods listed on this registry are uploaded by community members. Each uploader is
              solely responsible for the content they publish and warrants that they hold the rights
              to distribute it. The maintainers do not claim ownership of community submissions.
            </p>
            <p>
              Every uploaded mod version is scanned for malware and goes live only once the scan
              comes back clean. Guides are reviewed by a moderator before they are published, and
              an edit to a published guide goes back for review. Other mod content is not reviewed
              before it is listed: moderators act on reports and on what they find themselves.
            </p>
          </section>

          <section className="space-y-2">
            <h2 className="text-lg font-semibold text-foreground">2. What you may not upload</h2>
            <ul className="list-disc space-y-1 pl-5">
              <li>
                Copyrighted game assets — textures, models, audio, or other files extracted from
                Ravenswatch or any other game — redistributed on their own or in bulk.
              </li>
              <li>Pirated, cracked, or otherwise unlawfully obtained material.</li>
              <li>
                Anything that unlocks paid content (DLC heroes, outfits or other bought content)
                for players who do not own it.
              </li>
              <li>Malware, miners, or code intended to harm or deceive other users.</li>
              <li>Counterfeit goods, or content that infringes a third party&apos;s trademark.</li>
              <li>Illegal content, or content that violates the rights of others.</li>
            </ul>
            <p>
              Mods should ship original or properly-licensed assets and the declarative manifest the
              tooling produces. Overrides are applied to a user&apos;s own local game files; the
              registry is not a place to mirror the game&apos;s shipped content.
            </p>
          </section>

          <section className="space-y-2">
            <h2 className="text-lg font-semibold text-foreground">3. Fair play</h2>
            <p>
              Mods may change how the game plays, but not at the expense of other players. A mod
              that changes what the party plays (content, balance, enemies, rules) carries the{' '}
              <strong>Private games only</strong> label, set automatically when the mod is packed,
              and only a moderator can remove it. While such a mod is applied, the mod manager keeps
              public matchmaking closed, so it is only played with people who joined a private game.
            </p>
            <p>
              Mods built to gain an advantage over other players in public games, to get around
              that restriction, or to tamper with other players&apos; accounts, saves or progress
              are not allowed and are removed.
            </p>
          </section>

          <section className="space-y-2">
            <h2 className="text-lg font-semibold text-foreground">4. Trademarks</h2>
            <p>
              &quot;Ravenswatch&quot; and related marks are trademarks of Passtech Games. This is an
              unofficial, fan-made project and is not affiliated with or endorsed by Passtech Games
              or NACON. The game&apos;s name is used here only to describe compatibility (nominative
              fair use).
            </p>
          </section>

          <section className="space-y-2">
            <h2 className="text-lg font-semibold text-foreground">
              5. Reporting infringing content (DMCA / takedown)
            </h2>
            <p>
              If you are a rights holder and believe material on this site infringes your copyright
              or trademark, send a takedown notice and we will review and remove infringing content
              promptly. Please include:
            </p>
            <ul className="list-disc space-y-1 pl-5">
              <li>Identification of the work you claim is infringed.</li>
              <li>The exact URL(s) of the infringing material on this site.</li>
              <li>Your contact information.</li>
              <li>
                A statement that you have a good-faith belief the use is not authorised, and that
                the information in your notice is accurate.
              </li>
            </ul>
            <p>
              Submit notices via the project&apos;s{' '}
              <a
                href="https://github.com/Ravenswatch-Modding/RavenswatchModManager/issues/new"
                target="_blank"
                rel="noopener noreferrer"
                className="underline hover:text-foreground"
              >
                issue tracker
              </a>{' '}
              or the channels on the{' '}
              <Link href="/contact" className="underline hover:text-foreground">
                contact page
              </Link>
              . We also remove content that violates this policy on our own initiative, and we may
              suspend accounts that repeatedly infringe.
            </p>
            <p>
              For non-copyright issues — malware, stolen work, broken uploads, spam, or otherwise
              inappropriate content — use the <strong>Report</strong> button on the mod&apos;s page.
              Reports go straight to the moderation queue and each is reviewed by a moderator.
            </p>
            <p>
              A moderator who acts on a mod either hides it from the registry while the author fixes
              it, or removes it. The author is told why and can reply to the moderator from the
              mod&apos;s page. Accounts that keep breaking this policy can be banned from
              publishing.
            </p>
          </section>

          <section className="space-y-2">
            <h2 className="text-lg font-semibold text-foreground">6. Counter-notice</h2>
            <p>
              If your content was removed and you believe that was a mistake or misidentification,
              you may reply to the original notice with an explanation and your contact details, and
              we will reconsider.
            </p>
          </section>
        </CardContent>
      </Card>

      <p className="mt-8 text-center text-xs text-muted-foreground">
        <Link href="/" className="underline hover:text-foreground">
          Home
        </Link>
        {' · '}
        <Link href="/legal" className="underline hover:text-foreground">
          Legal Notice
        </Link>
        {' · '}
        <Link href="/privacy" className="underline hover:text-foreground">
          Privacy Policy
        </Link>
      </p>
    </main>
  );
}
