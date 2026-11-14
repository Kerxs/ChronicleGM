//! 开发工具：识别本机正在运行的模型服务。
//! 用法：cargo run -p chronicle-llm --example discover

fn main() {
    let t = std::time::Instant::now();
    let found = chronicle_llm::discovery::discover(&[], std::time::Duration::from_millis(600));
    println!("用时 {:.2}s，发现 {} 个服务", t.elapsed().as_secs_f32(), found.len());
    println!("{}", serde_json::to_string_pretty(&found).unwrap());
}
