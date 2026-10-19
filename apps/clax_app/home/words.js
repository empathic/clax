// The icon words an app's icon may name, as the launcher maps them to
// filled Material Symbols (lib/ui/launcher.dart `iconFor`), each row with
// the category a group of such apps is named after: [words, glyph,
// category].
export const WORDS = [
  [['chart', 'graph', 'stats', 'analytics'], 'insert_chart_fill', 'Stats'],
  [['calendar', 'schedule', 'date'], 'calendar_month_fill', 'Planning'],
  [['recipe', 'food', 'cooking', 'meal'], 'restaurant_fill', 'Food'],
  [['run', 'running', 'fitness', 'workout', 'exercise'], 'directions_run_fill', 'Fitness'],
  [['list', 'todo', 'checklist', 'tasks', 'task'], 'checklist_fill', 'Planning'],
  [['note', 'notes', 'journal', 'diary', 'text'], 'edit_note_fill', 'Notes'],
  [['money', 'budget', 'finance', 'expense', 'expenses'], 'savings_fill', 'Money'],
  [['shopping', 'cart', 'groceries', 'grocery'], 'shopping_cart_fill', 'Food'],
  [['timer', 'clock', 'time', 'stopwatch'], 'timer_fill', 'Tools'],
  [['map', 'location', 'travel', 'trip'], 'map_fill', 'Travel'],
  [['book', 'books', 'reading', 'library'], 'menu_book_fill', 'Reading'],
  [['music', 'song', 'audio'], 'music_note_fill', 'Media'],
  [['photo', 'image', 'camera', 'gallery'], 'photo_fill', 'Media'],
  [['game', 'games', 'puzzle'], 'sports_esports_fill', 'Games'],
  [['habit', 'habits', 'streak'], 'local_fire_department_fill', 'Habits'],
  [['water', 'drink'], 'water_drop_fill', 'Health'],
  [['sleep', 'moon'], 'bedtime_fill', 'Health'],
  [['health', 'medical', 'heart'], 'favorite_fill', 'Health'],
  [['weather', 'sun'], 'wb_sunny_fill', 'Weather'],
  [['home', 'house'], 'home_fill', 'Home'],
  [['people', 'contacts', 'team', 'person'], 'group_fill', 'People'],
  [['chat', 'message', 'mail', 'email'], 'chat_bubble_fill', 'People'],
  [['code', 'dev', 'terminal'], 'code_fill', 'Tools'],
  [['calculator', 'math'], 'calculate_fill', 'Tools'],
  [['dashboard', 'home-screen'], 'dashboard_fill', 'Tools'],
  [['plant', 'garden', 'plants'], 'local_florist_fill', 'Home'],
  [['pet', 'dog', 'cat'], 'pets_fill', 'Home'],
  [['car', 'drive', 'fuel'], 'directions_car_fill', 'Travel'],
  [['star', 'favorite', 'rating'], 'star_fill', 'Favourites'],
  [['settings', 'tool', 'tools'], 'build_fill', 'Tools'],
  [['app', 'apps', 'widget'], 'widgets_fill', 'Tools'],
];

/** An app's icon word, trimmed and in lower case, or "" when it has none. */
export const wordOf = (app) => (typeof app?.icon?.word === 'string' ? app.icon.word.trim().toLowerCase() : '');
