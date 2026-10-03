
REM * ************************************************************
REM * Title:     README.TXT
REM *
REM * Target Application:      N/A
REM * Application Version:   MMS Data Model v4.12
REM * ************************************************************
REM * Purpose:  Provide specific notes on the data contents of dvd archive 
REM *           
REM
REM * ************************************************************
REM * History: 
REM
REM * Date         Author      Comment
REM * ==========   ========    ==============================
REM * 11/07/2009   MMS         Initial Release
REM * ************************************************************

************ 
0. Summary
************ 
This file contaitns details ont eh following:

  1. Tables that contain static/standing data that cannot be loaded across time independantly
  

******************************** 
1. Static/Standing data tables

The public dvd archive iincludes the data for all public MMS Dat Model tables. The data
is distributed in monthly lots, however, some of the tables are not periodic in nature. 
These tables generally contain static data such as GST rates or standing data relating to
registration data. The tables include all data at the time the archive was created

If you wish to load data to these tables you will get errors reporting duplicate records if
you do not truncate existing data in the table.

The following tables are the Static/Standing data tables

	ANCILLARY_RECOVERY_SPLIT
	BIDDUIDDETAILSTRK
	BIDDUIDDETAILS
	BIDTYPESTRK
	BIDTYPES
	BILLINGCALENDAR
	BILLSMELTERRATE
	DISPATCHABLEUNIT
	DUDETAILSUMMARY
	DUDETAIL
	EMSMASTER
	GENERICEQUATIONDESC
	GST_BAS_CLASS
	GST_RATE
	GST_TRANSACTION_CLASS
	GST_TRANSACTION_TYPE
	INSTRUCTIONSUBTYPE
	INSTRUCTIONTYPE
	INTERCONNECTORCONSTRAINT
	INTERCONNECTOR
	IRFMAMOUNT
	IRFMEVENTS
	LOSSFACTORMODEL
	LOSSMODEL
	MARKETFEEDATA
	MARKETFEETRK
	MARKETFEE
	MARKETNOTICETYPE
	MARKETSUSPENSION
	MARKETSUSREGION
	MNSP_INTERCONNECTOR
	MNSP_PARTICIPANT
	OVERRIDERRP
	PARTICIPANTCATEGORYALLOC
	PARTICIPANTCATEGORY
	PARTICIPANTCLASS
	PARTICIPANT
	PREDISPATCHSCENARIODEMANDTRK
	PREDISPATCHSCENARIODEMAND
	REGIONAPCINTERVALS
	REGIONAPC
	REGIONSTANDINGDATA
	REGION
	STADUALLOC
	STATIONOPERATINGSTATUS
	STATIONOWNERTRK
	STATIONOWNER
	STATION

REM * ************************************************************
REM ********                 End Of README.TXT                       *******************
REM * ************************************************************



