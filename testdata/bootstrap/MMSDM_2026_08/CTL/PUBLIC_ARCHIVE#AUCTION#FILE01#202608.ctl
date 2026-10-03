-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#AUCTION#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS AUCTION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#AUCTION#FILE01#202608010000.CSV
APPEND INTO TABLE AUCTION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
AUCTIONID CHAR(30),
AUCTIONDATE DATE "YYYY/MM/DD HH24:MI:SS",
NOTIFYDATE DATE "YYYY/MM/DD HH24:MI:SS",
STARTDATE DATE "YYYY/MM/DD HH24:MI:SS",
ENDDATE DATE "YYYY/MM/DD HH24:MI:SS",
DESCRIPTION CHAR(100),
AUTHORISEDDATE DATE "YYYY/MM/DD HH24:MI:SS",
AUTHORISEDBY CHAR(30),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
