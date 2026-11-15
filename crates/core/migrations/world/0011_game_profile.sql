-- v11：剧本的「玩法配置」（玩法比重、属性方案、检定规则、叙事风格），JSON。
-- 空对象表示还没有配置：程序按题材关键词推断（profile::infer）。
ALTER TABLE scripts ADD COLUMN game_profile TEXT NOT NULL DEFAULT '{}';
