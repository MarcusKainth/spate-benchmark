# A dedicated VPC rather than the account default: the benchmark box shares a
# network with nothing, and the whole footprint is legible in one file. Three
# public subnets give the launcher's capacity-retry loop three AZs to try —
# c8g capacity is not uniform across zones. No NAT: the box needs outbound
# internet (apt, crates.io, Docker Hub, S3) and gets it via public IP through
# the internet gateway, which costs nothing while idle.

# Flow logs are deliberately absent: this VPC holds one ephemeral, egress-only
# box per run, every principal touching it is a CloudTrail-attributable role,
# and the interesting record — what the box did — ships to S3 as the run log.
# Enable them temporarily for network forensics if a run ever warrants it.
#trivy:ignore:AVD-AWS-0178
resource "aws_vpc" "bench" {
  cidr_block           = "10.42.0.0/16"
  enable_dns_support   = true
  enable_dns_hostnames = true

  tags = {
    Name = "spate-bench"
  }
}

resource "aws_internet_gateway" "bench" {
  vpc_id = aws_vpc.bench.id

  tags = {
    Name = "spate-bench"
  }
}

data "aws_availability_zones" "available" {
  state = "available"
}

# Public-on-launch is the design, not an oversight: there is no NAT (nothing
# to keep warm between runs), the box needs outbound to apt, crates.io, Docker
# Hub and S3, and its security group has zero inbound rules — a public IP with
# no listening ingress path is an address, not an exposure.
#trivy:ignore:AVD-AWS-0164
resource "aws_subnet" "public" {
  count = 3

  vpc_id                  = aws_vpc.bench.id
  cidr_block              = cidrsubnet(aws_vpc.bench.cidr_block, 4, count.index)
  availability_zone       = data.aws_availability_zones.available.names[count.index]
  map_public_ip_on_launch = true

  tags = {
    Name = "spate-bench-public-${count.index}"
  }
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.bench.id

  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.bench.id
  }

  tags = {
    Name = "spate-bench-public"
  }
}

resource "aws_route_table_association" "public" {
  count = 3

  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

# Egress-only. No SSH, no inbound anything: the box is driven entirely by its
# user-data and reports back through S3. Interactive access, when deliberately
# enabled, goes through SSM — which is also outbound-only.
resource "aws_security_group" "bench" {
  name        = "spate-bench"
  description = "spate-benchmark box: egress only"
  vpc_id      = aws_vpc.bench.id

  # Unrestricted egress is the deliberate trade: the box pulls from apt
  # mirrors, crates.io, Docker Hub and S3, none of which publish stable CIDRs
  # worth pinning. What bounds exfiltration is not this rule but what the box
  # can know — it holds no secrets, and its only credential writes to one S3
  # prefix.
  #trivy:ignore:AVD-AWS-0104
  egress {
    description = "all outbound"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name = "spate-bench"
  }
}
