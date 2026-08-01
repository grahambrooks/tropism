# TRAP: a spec requiring the code under test is not a cycle, and rspec is a
# development-group gem used only here.

require "rspec"

require_relative "../lib/shop/order"

RSpec.describe Shop::Order do
  it "keeps its total" do
    expect(described_class.new("x", 1000).total_cents).to eq(1000)
  end
end
