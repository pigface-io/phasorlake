-- ************************************************************
-- Title:	PUBLIC_DVD_AUCTION_IC_ALLOCATIONS_201910.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS AUCTION_IC_ALLOCATIONS owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_AUCTION_IC_ALLOCATIONS_201910010000.CSV
APPEND INTO TABLE AUCTION_IC_ALLOCATIONS
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
CONTRACTYEAR FLOAT EXTERNAL,
QUARTER FLOAT EXTERNAL,
VERSIONNO FLOAT EXTERNAL,
INTERCONNECTORID CHAR(10),
FROMREGIONID CHAR(10),
MAXIMUMUNITS FLOAT EXTERNAL,
PROPORTION FLOAT EXTERNAL,
AUCTIONFEE FLOAT EXTERNAL,
CHANGEDATE DATE "yyyy/mm/dd hh24:mi:ss",
CHANGEDBY CHAR(15),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
AUCTIONFEE_SALES FLOAT EXTERNAL)
-- End of Script
