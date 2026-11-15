-- 运行监控：首个 token 时间、提示词处理与生成速度、上下文长度、语音时长。拿不到的为空。
ALTER TABLE llm_requests ADD COLUMN first_token_ms INTEGER;
ALTER TABLE llm_requests ADD COLUMN prompt_per_second REAL;
ALTER TABLE llm_requests ADD COLUMN predicted_per_second REAL;
ALTER TABLE llm_requests ADD COLUMN context_length INTEGER;
ALTER TABLE llm_requests ADD COLUMN audio_ms INTEGER;
