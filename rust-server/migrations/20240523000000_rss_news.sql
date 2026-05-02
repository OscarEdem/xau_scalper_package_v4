CREATE TABLE IF NOT EXISTS rss_news (
    link TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    pub_date TEXT NOT NULL,
    source TEXT NOT NULL,
    image_url TEXT,
    author TEXT,
    fetched_at BIGINT NOT NULL
);

-- Ensure author column exists if the table was created before it was added
DO $$ 
BEGIN 
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name='rss_news' AND column_name='author') THEN
        ALTER TABLE rss_news ADD COLUMN author TEXT;
    END IF;
END $$;
