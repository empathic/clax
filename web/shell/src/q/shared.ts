// The page's one question feed and one inbox feed, which every surface of
// the page shows (`index.ts` starts them).
import { InboxFeed, QuestionFeed } from "./feed.svelte";

let feeds: { questions: QuestionFeed; inbox: InboxFeed } | null = null;

export const shared = () => (feeds ??= { questions: new QuestionFeed(), inbox: new InboxFeed() });
