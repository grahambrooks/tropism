using Xunit;

using Shop.Domain.Orders;

namespace Shop.Domain.Tests;

public class PricingTests
{
    [Fact]
    public void TotalsLines() => Assert.Equal(0m, new Order().Total());
}
