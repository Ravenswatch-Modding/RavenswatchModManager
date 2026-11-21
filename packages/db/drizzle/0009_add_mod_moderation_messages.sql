-- Private moderation thread (owner, co-authors, admins). IF NOT EXISTS: some
-- databases are managed with `db:push`, which may have created it already.
CREATE TABLE IF NOT EXISTS "mod_moderation_messages" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"mod_id" uuid NOT NULL REFERENCES "mods"("id") ON DELETE cascade,
	"author_id" text REFERENCES "user"("id") ON DELETE set null,
	"kind" varchar(16) NOT NULL,
	"body" text,
	"from_status" varchar(16),
	"to_status" varchar(16),
	"from_moderator" boolean DEFAULT false NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "mod_moderation_messages_mod_idx"
  ON "mod_moderation_messages" ("mod_id", "created_at");
