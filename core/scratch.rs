use robotstxt::DefaultMatcher;

fn main() {
    let mut matcher = DefaultMatcher::default();
    let is_allowed = matcher.one_agent_allowed_by_robots("User-agent: *\nDisallow: /", "*", "http://example.com/foo");
    println!("Allowed: {}", is_allowed);
}
