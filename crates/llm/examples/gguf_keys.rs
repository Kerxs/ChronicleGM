//! 打印 GGUF 中与显存估算相关的字段（只读）。用法：cargo run -p chronicle-llm --example gguf_keys -- <文件>
fn main() {
    let p = std::env::args().nth(1).expect("文件路径");
    let i = chronicle_llm::gguf::read_gguf(std::path::Path::new(&p)).unwrap();
    println!("arch={:?} blocks={:?} emb={:?} heads={:?} kv_heads={:?} k={:?} v={:?} full_attn_interval={:?} experts={:?} size={} MB",
        i.architecture, i.block_count, i.embedding_length, i.head_count, i.head_count_kv, i.key_length, i.value_length, i.full_attention_interval, i.expert_count, i.file_size >> 20);
}
